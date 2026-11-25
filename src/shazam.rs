//! Shazam acoustic fingerprint generation and Discovery API query client.

use std::array;
use std::f64::consts::PI;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crc32fast::Hasher;
use realfft::{RealFftPlanner, RealToComplex};
use reqwest::{Client, RequestBuilder};
use rustfft::num_complex::Complex;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::audio::{PcmTap, Snapshot, SAMPLE_RATE_HZ};
use crate::timeline::Alignment;
use crate::track::TrackName;
use crate::{http, log, text};

const DATA_URI_PREFIX: &str = "data:audio/vnd.shazam.sig;base64,";

/// Computes 2048-point Hanning window coefficients (2050-sample window with boundary endpoints omitted).
fn hanning_window() -> &'static [f32; 2048] {
    static WINDOW: OnceLock<[f32; 2048]> = OnceLock::new();
    WINDOW.get_or_init(|| {
        array::from_fn(|i| {
            let angle = 2.0 * PI * (i + 1) as f64 / 2049.0;
            (0.5 * (1.0 - angle.cos())) as f32
        })
    })
}

/// Shazam acoustic match result: identified song, track keys, and aligned timeline offsets.
#[derive(Debug, Clone, PartialEq)]
pub struct SongMatch {
    pub track: TrackName,
    /// Shazam track identifier.
    pub key: String,
    /// Apple Music catalog identifier used to query recording duration.
    pub apple_music_id: Option<String>,
    /// Reference recording alignments and playback offsets returned by Shazam.
    pub alignments: Vec<Alignment>,
    /// Estimated audible timestamp of the sample's starting instant.
    pub heard_at: Instant,
    /// True if timing is derived from direct loopback capture rather than estimated via ffmpeg.
    pub exact_timing: bool,
}

/// The Apple Music id of the recording in Shazam's `track`, from its "play in Apple Music"
/// action.
fn apple_music_id(track: &Value) -> Option<String> {
    let actions = track.pointer("/hub/actions")?.as_array()?;
    let play = actions
        .iter()
        .find(|action| action.get("type").and_then(Value::as_str) == Some("applemusicplay"))?;
    let id = play.get("id")?.as_str()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// The result of an acoustic query to Shazam Discovery.
#[derive(Debug, Clone, PartialEq)]
pub enum ShazamResult {
    /// Shazam identified a song.
    Match(SongMatch),
    /// Shazam processed the sample but did not detect any song playing (silence, speech, ad).
    /// It says how long to wait before asking again; asking sooner gets requests refused.
    NoMatch { retry_after: Option<Duration> },
    /// Too little audio had been captured to fingerprint (the stream is still starting, or has
    /// stalled), so nothing was sent to Shazam. Not a failure: there is nothing to back off from.
    NoAudio,
    /// Network failure, timeout, refusal or invalid response. A refusal (`429 Too Many
    /// Requests`) may say how long to wait.
    Error { retry_after: Option<Duration> },
}

impl Default for ShazamResult {
    /// What a query counts as when its task panicked.
    fn default() -> Self {
        Self::Error { retry_after: None }
    }
}

/// Reads Shazam's answer to a sample whose first moment was audible at `heard_at`.
fn parse_answer(payload: &Value, heard_at: Instant, exact_timing: bool) -> ShazamResult {
    let retry_after = payload
        .get("retryms")
        .and_then(Value::as_u64)
        .map(Duration::from_millis);
    let no_match = ShazamResult::NoMatch { retry_after };
    let Some(track_json) = payload.get("track") else {
        return no_match;
    };
    let Some(title) = track_json.get("title").and_then(Value::as_str) else {
        return no_match;
    };
    let artist = track_json
        .get("subtitle")
        .and_then(Value::as_str)
        .unwrap_or("");
    let track = TrackName::new(title, artist);
    if track.title.is_empty() {
        return no_match;
    }

    // A match without a position says nothing about where playback is, so it is left out
    let alignments = payload
        .get("matches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|matched| {
            Some(Alignment {
                recording: matched.get("id")?.as_str()?.to_string(),
                offset: matched.get("offset")?.as_f64()?,
            })
        })
        .collect();

    ShazamResult::Match(SongMatch {
        track,
        key: track_json
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        apple_music_id: apple_music_id(track_json),
        alignments,
        heard_at,
        exact_timing,
    })
}

#[derive(Clone, Copy, Debug)]
struct FrequencyPeak {
    fft_pass_number: u32,
    peak_magnitude: u16,
    corrected_peak_frequency_bin: u16,
}

#[derive(Clone, Copy, Debug)]
enum FrequencyBand {
    Band250To520 = 0,
    Band520To1450 = 1,
    Band1450To3500 = 2,
    Band3500To5500 = 3,
}

pub struct DecodedSignature {
    pub sample_rate_hz: u32,
    pub number_samples: u32,
    frequency_band_to_sound_peaks: [Vec<FrequencyPeak>; 4],
}

impl DecodedSignature {
    pub fn encode_to_binary(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(512);
        let push_u32 =
            |bytes: &mut Vec<u8>, value: u32| bytes.extend_from_slice(&value.to_le_bytes());

        let rate_id: u32 = match self.sample_rate_hz {
            8000 => 1,
            11025 => 2,
            16000 => 3,
            32000 => 4,
            44100 => 5,
            48000 => 6,
            _ => 3,
        };

        // 48-byte outer header
        push_u32(&mut bytes, 0xcafe2580); // magic1
        push_u32(&mut bytes, 0); // crc32 placeholder (offset 4..8)
        push_u32(&mut bytes, 0); // size_minus_header placeholder (offset 8..12)
        push_u32(&mut bytes, 0x94119c00); // magic2
        bytes.extend_from_slice(&[0u8; 12]); // void1
        push_u32(&mut bytes, rate_id << 27);
        bytes.extend_from_slice(&[0u8; 8]); // void2
        push_u32(
            &mut bytes,
            self.number_samples + (self.sample_rate_hz as f32 * 0.24) as u32,
        );
        push_u32(&mut bytes, (15 << 19) + 0x40000);

        // 8-byte payload sub-header
        push_u32(&mut bytes, 0x40000000);
        push_u32(&mut bytes, 0); // size_minus_header placeholder (offset 52..56)

        for (band_index, peaks) in self.frequency_band_to_sound_peaks.iter().enumerate() {
            if peaks.is_empty() {
                continue;
            }
            let mut peak_bytes = Vec::with_capacity(peaks.len() * 5);
            let mut fft_pass_number = 0u32;

            for peak in peaks {
                if peak.fft_pass_number - fft_pass_number >= 255 {
                    peak_bytes.push(0xff);
                    peak_bytes.extend_from_slice(&peak.fft_pass_number.to_le_bytes());
                    fft_pass_number = peak.fft_pass_number;
                }
                peak_bytes.push((peak.fft_pass_number - fft_pass_number) as u8);
                peak_bytes.extend_from_slice(&peak.peak_magnitude.to_le_bytes());
                peak_bytes.extend_from_slice(&peak.corrected_peak_frequency_bin.to_le_bytes());
                fft_pass_number = peak.fft_pass_number;
            }

            push_u32(&mut bytes, 0x60030040 + band_index as u32);
            push_u32(&mut bytes, peak_bytes.len() as u32);
            bytes.extend_from_slice(&peak_bytes);
            let padding = (4 - (peak_bytes.len() % 4)) % 4;
            bytes.resize(bytes.len() + padding, 0);
        }

        let size_minus_header = ((bytes.len() - 48) as u32).to_le_bytes();
        bytes[8..12].copy_from_slice(&size_minus_header);
        bytes[52..56].copy_from_slice(&size_minus_header);

        let mut hasher = Hasher::new();
        hasher.update(&bytes[8..]);
        bytes[4..8].copy_from_slice(&hasher.finalize().to_le_bytes());

        bytes
    }

    pub fn encode_to_uri(&self) -> String {
        use base64::Engine;

        format!(
            "{DATA_URI_PREFIX}{}",
            base64::prelude::BASE64_STANDARD.encode(self.encode_to_binary())
        )
    }
}

struct SignatureGenerator {
    ring_buffer_of_samples: Vec<i16>,
    ring_buffer_of_samples_index: usize,
    reordered_ring_buffer_of_samples: Vec<f32>,
    complex_fft_output: Vec<Complex<f32>>,
    fft_outputs: Vec<[f32; 1025]>,
    fft_outputs_index: u8,
    fft: Arc<dyn RealToComplex<f32>>,
    spread_fft_outputs: Vec<[f32; 1025]>,
    spread_fft_outputs_index: u8,
    spread_ffts_done: u32,
    signature: DecodedSignature,
}

impl SignatureGenerator {
    pub fn make_signature_from_pcm(s16_mono_16khz: &[i16]) -> DecodedSignature {
        use rustfft::num_traits::Zero;

        let mut planner = RealFftPlanner::<f32>::new();
        let mut this = Self {
            ring_buffer_of_samples: vec![0i16; 2048],
            ring_buffer_of_samples_index: 0,
            reordered_ring_buffer_of_samples: vec![0.0f32; 2048],
            complex_fft_output: vec![Complex::zero(); 1025],
            fft_outputs: vec![[0.0f32; 1025]; 256],
            fft_outputs_index: 0,
            fft: planner.plan_fft_forward(2048),
            spread_fft_outputs: vec![[0.0f32; 1025]; 256],
            spread_fft_outputs_index: 0,
            spread_ffts_done: 0,
            signature: DecodedSignature {
                sample_rate_hz: SAMPLE_RATE_HZ,
                number_samples: s16_mono_16khz.len() as u32,
                frequency_band_to_sound_peaks: Default::default(),
            },
        };

        for chunk in s16_mono_16khz.as_chunks::<128>().0 {
            this.do_fft(chunk);
            this.do_peak_spreading();
            this.spread_ffts_done += 1;
            if this.spread_ffts_done >= 46 {
                this.do_peak_recognition();
            }
        }

        this.signature
    }

    fn do_fft(&mut self, s16_chunk: &[i16; 128]) {
        self.ring_buffer_of_samples
            [self.ring_buffer_of_samples_index..self.ring_buffer_of_samples_index + 128]
            .copy_from_slice(s16_chunk);
        self.ring_buffer_of_samples_index = (self.ring_buffer_of_samples_index + 128) & 2047;

        let window = hanning_window();
        for (index, &multiplier) in window.iter().enumerate() {
            self.reordered_ring_buffer_of_samples[index] = self.ring_buffer_of_samples
                [(index + self.ring_buffer_of_samples_index) & 2047]
                as f32
                * multiplier;
        }

        let _ = self.fft.process(
            &mut self.reordered_ring_buffer_of_samples,
            &mut self.complex_fft_output,
        );

        let real_fft_results = &mut self.fft_outputs[self.fft_outputs_index as usize];
        for (result, complex) in real_fft_results
            .iter_mut()
            .zip(self.complex_fft_output.iter())
        {
            *result =
                ((complex.re.powi(2) + complex.im.powi(2)) / ((1 << 17) as f32)).max(0.0000000001);
        }

        self.fft_outputs_index = self.fft_outputs_index.wrapping_add(1);
    }

    fn do_peak_spreading(&mut self) {
        let real_fft_results = &self.fft_outputs[self.fft_outputs_index.wrapping_sub(1) as usize];
        let spread_fft_results =
            &mut self.spread_fft_outputs[self.spread_fft_outputs_index as usize];

        spread_fft_results.copy_from_slice(real_fft_results);
        for position in 0..=1022 {
            spread_fft_results[position] = spread_fft_results[position]
                .max(spread_fft_results[position + 1])
                .max(spread_fft_results[position + 2]);
        }

        let spread_copy = *spread_fft_results;
        for ffts_back in [1u8, 3, 6] {
            let former_output = &mut self.spread_fft_outputs
                [self.spread_fft_outputs_index.wrapping_sub(ffts_back) as usize];
            let bins = former_output.iter_mut().zip(spread_copy.iter());
            for (former, &spread) in bins {
                *former = former.max(spread);
            }
        }

        self.spread_fft_outputs_index = self.spread_fft_outputs_index.wrapping_add(1);
    }

    fn do_peak_recognition(&mut self) {
        let fft_minus_46 = &self.fft_outputs[self.fft_outputs_index.wrapping_sub(46) as usize];
        let fft_minus_49 =
            &self.spread_fft_outputs[self.spread_fft_outputs_index.wrapping_sub(49) as usize];

        for bin in 10..=1014 {
            if fft_minus_46[bin] >= 1.0 / 64.0 && fft_minus_46[bin] >= fft_minus_49[bin - 1] {
                let mut max_neighbor_in_49: f32 = 0.0;
                for offset in [-10i32, -7, -4, -3, 1, 2, 5, 8] {
                    max_neighbor_in_49 =
                        max_neighbor_in_49.max(fft_minus_49[(bin as i32 + offset) as usize]);
                }

                if fft_minus_46[bin] > max_neighbor_in_49 {
                    let mut max_neighbor_other = max_neighbor_in_49;
                    for other_offset in [
                        -53i32, -45, 165, 172, 179, 186, 193, 200, 214, 221, 228, 235, 242, 249,
                    ] {
                        let other_fft = &self.spread_fft_outputs[((self.spread_fft_outputs_index
                            as i32
                            + other_offset)
                            & 255)
                            as usize];
                        max_neighbor_other = max_neighbor_other.max(other_fft[bin - 1]);
                    }

                    if fft_minus_46[bin] > max_neighbor_other {
                        let fft_pass_number = self.spread_ffts_done - 46;

                        let log_magnitude = |at_bin: usize| {
                            fft_minus_46[at_bin].ln().max(1.0 / 64.0) * 1477.3 + 6144.0
                        };
                        let peak_magnitude = log_magnitude(bin);
                        let magnitude_before = log_magnitude(bin - 1);
                        let magnitude_after = log_magnitude(bin + 1);

                        let variation_1 = peak_magnitude * 2.0 - magnitude_before - magnitude_after;
                        if variation_1 <= 0.0 {
                            continue;
                        }
                        let variation_2 = (magnitude_after - magnitude_before) * 32.0 / variation_1;
                        let corrected_bin = ((bin as i32 * 64) + (variation_2 as i32)) as u16;

                        let frequency_hz = corrected_bin as f32 * (16000.0 / 2.0 / 1024.0 / 64.0);
                        let band = match frequency_hz as i32 {
                            250..=519 => FrequencyBand::Band250To520,
                            520..=1449 => FrequencyBand::Band520To1450,
                            1450..=3499 => FrequencyBand::Band1450To3500,
                            3500..=5500 => FrequencyBand::Band3500To5500,
                            _ => continue,
                        };

                        self.signature.frequency_band_to_sound_peaks[band as usize].push(
                            FrequencyPeak {
                                fft_pass_number,
                                peak_magnitude: peak_magnitude as u16,
                                corrected_peak_frequency_bin: corrected_bin,
                            },
                        );
                    }
                }
            }
        }
    }
}

/// Top-level fields of Shazam's `/discovery/v5/.../track/{key}` response.
#[derive(Deserialize, Default)]
#[serde(default)]
struct TrackSummary {
    title: String,
    subtitle: String,
}

/// Timeout for the optional `en-US` track-name fallback lookup.
const TRACK_NAME_TIMEOUT: Duration = Duration::from_secs(2);

/// Converts Shazam's `en-US` track summary into a Latin-script `TrackName`, returning `None`
/// if the artist is still empty or maps back to CJK (e.g. a known Chinese artist alias).
fn parse_track_summary(summary: TrackSummary) -> Option<TrackName> {
    let track = TrackName::new(&summary.title, &summary.subtitle);
    (!track.title.is_empty() && !track.artist.is_empty() && !text::contains_cjk(&track.artist))
        .then_some(track)
}

/// Attaches the iOS Shazam client headers shared across Discovery endpoints.
fn shazam_headers(request: RequestBuilder, accept_language: &str) -> RequestBuilder {
    request
        .header("X-Shazam-Platform", "IPHONE")
        .header("X-Shazam-AppVersion", "14.1.0")
        .header("Accept", "*/*")
        .header("Accept-Language", accept_language)
        .header(
            "User-Agent",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15",
        )
}

/// Fetches Shazam's `en-US` title and artist for track `key`, used when the primary `zh-CN`
/// query returned a transliterated Western artist and no lyrics catalogue matched it.
pub async fn english_track_name(client: &Client, key: &str) -> Option<TrackName> {
    if key.is_empty() {
        return None;
    }
    let url = format!(
        "https://amp.shazam.com/discovery/v5/en-US/US/iphone/-/track/{}?shazamapiversion=v3&video=v3",
        urlencoding::encode(key)
    );
    let request = shazam_headers(
        client.get(&url).timeout(TRACK_NAME_TIMEOUT),
        "en-US,en;q=0.9",
    );
    let summary: TrackSummary = http::get_json(request, "shazam").await?;
    parse_track_summary(summary)
}

/// Fingerprints the last 12 seconds of captured audio and asks Shazam what is playing.
pub async fn identify(client: &Client, pcm: &PcmTap) -> ShazamResult {
    let Some(snapshot) = wait_for_sample(pcm).await else {
        return ShazamResult::NoAudio;
    };
    ask(client, snapshot).await
}

/// Fingerprints `snapshot` and asks Shazam what is playing.
async fn ask(client: &Client, snapshot: Snapshot) -> ShazamResult {
    let Snapshot {
        samples,
        audible_at,
        exact_timing,
    } = snapshot;
    // Compute acoustic fingerprint off the async executor thread
    let fingerprint = tokio::task::spawn_blocking(move || {
        let signature = SignatureGenerator::make_signature_from_pcm(&samples);
        let milliseconds =
            (signature.number_samples as u64 * 1000) / signature.sample_rate_hz as u64;
        Fingerprint {
            uri: signature.encode_to_uri(),
            sample_milliseconds: milliseconds as u32,
        }
    });
    let fingerprint = match fingerprint.await {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            log::error(format!("shazam: fingerprinting failed: {error}"));
            return ShazamResult::default();
        }
    };

    let uuid1 = Uuid::new_v4().to_string().to_uppercase();
    let uuid2 = Uuid::new_v4().to_string().to_uppercase();
    let url = format!(
        "https://amp.shazam.com/discovery/v5/zh-CN/CN/iphone/-/tag/{uuid1}/{uuid2}\
         ?sync=true&webv3=true&sampling=true&connected=&shazamapiversion=v3\
         &sharehub=true&hubv5minorversion=v5.1&hidelb=true&video=v3"
    );

    let timestamp_milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let body = json!({
        "timezone": "Asia/Shanghai",
        "signature": {
            "uri": fingerprint.uri,
            "samplems": fingerprint.sample_milliseconds,
        },
        "timestamp": timestamp_milliseconds,
        "context": {},
        "geolocation": {},
    });

    let request = shazam_headers(client.post(&url), "zh-CN,zh;q=0.9,en-US;q=0.5").json(&body);
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            log::error(format!("shazam: {error}"));
            return ShazamResult::default();
        }
    };
    // A refusal for asking too often may say how long to stay away
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok()?.trim().parse().ok())
        .map(Duration::from_secs);
    let answer: reqwest::Result<Value> = async { response.error_for_status()?.json().await }.await;
    match answer {
        Ok(payload) => parse_answer(&payload, audible_at, exact_timing),
        Err(error) => {
            log::error(format!("shazam: {error}"));
            ShazamResult::Error { retry_after }
        }
    }
}

/// A sample's signature as Shazam takes it, and how long the sample is.
struct Fingerprint {
    uri: String,
    sample_milliseconds: u32,
}

/// Minimum audio duration (in seconds) required for a reliable Shazam acoustic match.
const MIN_SAMPLE_SECONDS: usize = 3;

/// Waits for at least 3 seconds of audio to accumulate (for immediate startup recognition),
/// returning the full available buffer (up to 12 seconds).
async fn wait_for_sample(pcm: &PcmTap) -> Option<Snapshot> {
    let min_samples = SAMPLE_RATE_HZ as usize * MIN_SAMPLE_SECONDS;
    for _ in 0..70 {
        if let Some(snapshot) = pcm.snapshot(min_samples) {
            return Some(snapshot);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_encoding() {
        let window = hanning_window();
        assert!((window[0] - 0.0000023508).abs() < 1e-9);
        assert!((window[0] - window[2047]).abs() < 1e-9);
        assert!(window[1023] > 0.9999);

        // Generate 2 seconds of a synthetic 440Hz + 880Hz signal at 16kHz
        let samples: Vec<i16> = (0..(16_000 * 2))
            .map(|i| {
                let time = i as f32 / 16_000.0;
                let wave = (2.0 * std::f32::consts::PI * 440.0 * time).sin() * 0.5
                    + (2.0 * std::f32::consts::PI * 880.0 * time).sin() * 0.3;
                (wave * 16_000.0) as i16
            })
            .collect();

        let signature = SignatureGenerator::make_signature_from_pcm(&samples);
        assert_eq!(signature.sample_rate_hz, 16_000);
        assert_eq!(signature.number_samples, 32_000);

        let binary = signature.encode_to_binary();
        assert!(binary.len() >= 56);

        let magic1 = u32::from_le_bytes(binary[0..4].try_into().unwrap());
        let expected_crc = u32::from_le_bytes(binary[4..8].try_into().unwrap());
        let size_minus_header = u32::from_le_bytes(binary[8..12].try_into().unwrap());
        let magic2 = u32::from_le_bytes(binary[12..16].try_into().unwrap());

        assert_eq!(magic1, 0xcafe2580);
        assert_eq!(magic2, 0x94119c00);
        assert_eq!(size_minus_header as usize, binary.len() - 48);

        let mut hasher = Hasher::new();
        hasher.update(&binary[8..]);
        assert_eq!(expected_crc, hasher.finalize());

        let uri = signature.encode_to_uri();
        assert!(uri.starts_with(DATA_URI_PREFIX));
    }

    #[test]
    fn test_apple_music_id_comes_from_the_play_action() {
        // The actions in Shazam's answer for Chris Yu's "地下铁"
        let track = json!({"title": "地下铁", "hub": {"actions": [
            {"name": "apple", "type": "applemusicplay", "id": "321101782"},
            {"name": "apple", "type": "uri", "uri": "https://audio-ssl.itunes.apple.com/preview.m4a"}
        ]}});
        assert_eq!(apple_music_id(&track).as_deref(), Some("321101782"));
        assert_eq!(apple_music_id(&json!({"title": "No hub"})), None);
    }

    #[test]
    fn test_answer_lists_every_recording_with_its_own_position() {
        // Track's own recording appears third, 37.5 s offset from the first two matches
        let answer = json!({
            "matches": [
                {"id": "628935208", "offset": 7.834339355, "timeskew": 0.009206653},
                {"id": "638814137", "offset": 7.881190917},
                {"id": "628935200", "offset": 45.339699218},
                {"id": "no-position"}
            ],
            "tagid": "9F659642-E2FB-4B69-ACAB-13FA163DBC7C",
            "track": {
                "key": "628935200",
                "title": "I'm Good (Blue)",
                "subtitle": "David Guetta & Bebe Rexha",
                "hub": {"actions": [{"name": "apple", "type": "applemusicplay", "id": "1678871244"}]}
            }
        });
        let heard_at = Instant::now();
        let ShazamResult::Match(matched) = parse_answer(&answer, heard_at, true) else {
            panic!("expected a match");
        };
        assert_eq!(matched.key, "628935200");
        assert_eq!(matched.apple_music_id.as_deref(), Some("1678871244"));
        let recordings: Vec<&str> = matched
            .alignments
            .iter()
            .map(|alignment| alignment.recording.as_str())
            .collect();
        assert_eq!(recordings, ["628935208", "638814137", "628935200"]);
        assert!((matched.alignments[2].offset - 45.3397).abs() < 1e-3);
    }

    #[test]
    fn test_no_match_answer_says_when_to_ask_again() {
        // Shazam's actual answer to a sample of speech
        let answer = json!({"matches": [], "tagid": "466C517C", "retryms": 9000});
        assert_eq!(
            parse_answer(&answer, Instant::now(), true),
            ShazamResult::NoMatch {
                retry_after: Some(Duration::from_secs(9))
            }
        );
    }

    #[test]
    fn test_english_track_summary_keeps_latin_artists_and_skips_cjk_ones() {
        let western: TrackSummary =
            serde_json::from_str(r#"{"title": "1-2-3", "subtitle": "Gloria Estefan"}"#).unwrap();
        assert_eq!(
            parse_track_summary(western),
            Some(TrackName::new("1-2-3", "Gloria Estefan"))
        );

        // Known Chinese artist aliases map back to CJK via `TrackName::new` and are skipped
        let chinese_alias: TrackSummary =
            serde_json::from_str(r#"{"title": "Mojito", "subtitle": "Jay Chou"}"#).unwrap();
        assert_eq!(parse_track_summary(chinese_alias), None);

        let incomplete: TrackSummary =
            serde_json::from_str(r#"{"title": "1-2-3", "subtitle": ""}"#).unwrap();
        assert_eq!(parse_track_summary(incomplete), None);
    }
}
