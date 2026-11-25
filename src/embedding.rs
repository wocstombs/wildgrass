//! Embedded character/subword n-gram vector model for candidate song ranking.

use crate::text;

/// Dimensionality of the local character/subword n-gram embedding space.
pub const EMBEDDING_DIMENSIONS: usize = 256;

/// Minimum embedding similarity score required to accept a lyric candidate.
pub const MIN_CANDIDATE_SCORE: f32 = 0.40;

/// Projects `text` into an L2-normalized 256-D character/subword n-gram vector using signed
/// SplitMix64 feature hashing (unigrams, bigrams, bounded trigrams, and whole tokens).
pub fn embed(text: &str) -> [f32; EMBEDDING_DIMENSIONS] {
    let mut embedding = [0.0f32; EMBEDDING_DIMENSIONS];
    let normalized = text::normalize_text(text);
    if normalized.is_empty() {
        return embedding;
    }

    let characters: Vec<char> = normalized.chars().collect();

    // Character unigrams
    for &character in &characters {
        if character == ' ' {
            continue;
        }
        let weight = if text::is_cjk_char(character) {
            1.25
        } else {
            0.35
        };
        let hash = mix64(0x1000_0000_0000_0001 ^ (character as u64));
        accumulate(&mut embedding, hash, weight);
    }

    // Character bigrams (skipping across spaces)
    let non_space: Vec<char> = characters.iter().copied().filter(|&c| c != ' ').collect();
    for pair in non_space.windows(2) {
        let key = ((pair[0] as u64) << 21) ^ (pair[1] as u64) ^ 0x2000_0000_0000_0002;
        accumulate(&mut embedding, mix64(key), 1.5);
    }

    // Whole-word tokens and bounded character trigrams
    for token in normalized.split_whitespace() {
        // Whole-word token hash
        let mut word_hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in token.as_bytes() {
            word_hash ^= *byte as u64;
            word_hash = word_hash.wrapping_mul(0x0100_0000_01b3);
        }
        accumulate(&mut embedding, mix64(word_hash), 2.2);

        // Bounded character trigrams: ^ + token + $
        let mut bounded = Vec::with_capacity(token.chars().count() + 2);
        bounded.push('^');
        bounded.extend(token.chars());
        bounded.push('$');

        for trigram in bounded.windows(3) {
            let key = ((trigram[0] as u64) << 42)
                ^ ((trigram[1] as u64) << 21)
                ^ (trigram[2] as u64)
                ^ 0x3000_0000_0000_0003;
            accumulate(&mut embedding, mix64(key), 1.8);
        }
    }

    // L2 normalization
    let squared_length: f32 = embedding.iter().map(|x| x * x).sum();
    if squared_length > 1e-12 {
        let inverse_length = 1.0 / squared_length.sqrt();
        for x in &mut embedding {
            *x *= inverse_length;
        }
    }
    embedding
}

/// Computes the cosine similarity in `[-1.0, 1.0]` between two L2-normalized embeddings.
pub fn cosine_similarity(a: &[f32; EMBEDDING_DIMENSIONS], b: &[f32; EMBEDDING_DIMENSIONS]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Computes a field-aware semantic similarity score in `[0.0, 1.0]` between an ICY/search
/// query (`query_title`, `query_artist`) and a lyric database candidate
/// (`candidate_title`, `candidate_artist`).
///
/// Automatically handles stations that broadcast `"Title - Artist"` in reverse order by
/// evaluating both the direct and swapped field alignments.
pub fn score_candidate(
    query_title: &str,
    query_artist: &str,
    candidate_title: &str,
    candidate_artist: &str,
) -> f32 {
    // Opposite directions count as nothing in common
    let similarity = |a: &[f32; EMBEDDING_DIMENSIONS], b: &[f32; EMBEDDING_DIMENSIONS]| {
        cosine_similarity(a, b).max(0.0)
    };
    let query_title_embedding = embed(query_title);
    let candidate_title_embedding = embed(candidate_title);
    let candidate_artist_embedding = embed(candidate_artist);

    let full_query = format!("{query_title} {query_artist}");
    let full_candidate = format!("{candidate_title} {candidate_artist}");
    let full_similarity = similarity(&embed(&full_query), &embed(&full_candidate));

    let direct_title_similarity = similarity(&query_title_embedding, &candidate_title_embedding);

    let field_score = if query_artist.trim().is_empty() {
        0.70 * direct_title_similarity + 0.30 * full_similarity
    } else {
        // How well the fields match when lined up one way round
        let aligned = |title_similarity: f32, artist_similarity: f32| {
            0.55 * title_similarity + 0.25 * artist_similarity + 0.20 * full_similarity
        };
        let query_artist_embedding = embed(query_artist);
        let direct_artist_similarity =
            similarity(&query_artist_embedding, &candidate_artist_embedding);
        let direct = aligned(direct_title_similarity, direct_artist_similarity);

        // Swapped alignment in case the radio ICY string was "Title - Artist"
        let swapped_title_similarity =
            similarity(&query_artist_embedding, &candidate_title_embedding);
        let swapped_artist_similarity =
            similarity(&query_title_embedding, &candidate_artist_embedding);
        let swapped = aligned(swapped_title_similarity, swapped_artist_similarity);

        let mut best = direct.max(swapped);

        // Cross-script tolerance: when the artist matches and query title is Latin/Pinyin
        // while candidate title is Chinese, allow the candidate if the artist is confident.
        let artist_matches = direct_artist_similarity >= 0.80 || swapped_artist_similarity >= 0.80;
        if artist_matches
            && (!text::contains_cjk(query_title) && text::contains_cjk(candidate_title))
        {
            best = best.max(MIN_CANDIDATE_SCORE + 0.05);
        }

        best
    };

    // Penalize instrumental/karaoke/backing-track candidates unless the query asks for one
    let query_wants_instrumental = text::has_instrumental_keyword(&full_query);
    let candidate_is_instrumental = text::has_instrumental_keyword(&full_candidate);

    if !query_wants_instrumental && candidate_is_instrumental {
        (field_score - 0.35).max(0.0)
    } else {
        field_score
    }
}

#[inline]
fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[inline]
fn accumulate(embedding: &mut [f32; EMBEDDING_DIMENSIONS], hash: u64, weight: f32) {
    let first_index = (hash as usize) & (EMBEDDING_DIMENSIONS - 1);
    let first_contribution = if (hash & 0x100) != 0 { weight } else { -weight };
    embedding[first_index] += first_contribution;

    let second_hash = hash >> 16;
    let second_index = (second_hash as usize) & (EMBEDDING_DIMENSIONS - 1);
    let second_contribution = if (second_hash & 0x100) != 0 {
        weight * 0.5
    } else {
        -weight * 0.5
    };
    embedding[second_index] += second_contribution;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_embedding_ranks_correct_candidate_and_handles_swapped_icy() {
        // Standard "Artist - Title" ICY metadata
        let exact_score = score_candidate(
            "Cruel Summer",
            "Taylor Swift",
            "Cruel Summer",
            "Taylor Swift",
        );
        let wrong_score = score_candidate(
            "Cruel Summer",
            "Taylor Swift",
            "Shake It Off",
            "Taylor Swift",
        );
        let instrumental_score = score_candidate(
            "Cruel Summer",
            "Taylor Swift",
            "Cruel Summer (Karaoke Instrumental)",
            "Taylor Swift",
        );

        assert!(
            exact_score > 0.95,
            "exact match should be near 1.0, got {exact_score}"
        );
        assert!(exact_score > wrong_score + 0.30);
        assert!(exact_score > instrumental_score + 0.25);

        // Swapped "Title - Artist" ICY metadata (e.g. "大鱼 - 周深" parsed with artist="大鱼", title="周深")
        let swapped_score = score_candidate("周深", "大鱼", "大鱼", "周深");
        let unrelated_cjk = score_candidate("周深", "大鱼", "孤勇者", "陈奕迅");
        assert!(
            swapped_score > 0.85,
            "swapped fields should still score high, got {swapped_score}"
        );
        assert!(swapped_score > unrelated_cjk + 0.50);

        // Chinese instrumental tag ("纯音乐") is penalized for sung songs, but not when asked for
        let sung = score_candidate("奉献", "苏芮", "奉献", "苏芮");
        let chinese_instrumental = score_candidate("奉献", "苏芮", "奉献 (纯音乐)", "苏芮");
        let asked_for_instrumental =
            score_candidate("奉献 (纯音乐)", "苏芮", "奉献 (纯音乐)", "苏芮");
        assert!(sung > chinese_instrumental + 0.25);
        assert!(asked_for_instrumental > 0.95);
    }
}
