//! Traditional Chinese to Simplified Chinese transliteration and pinyin/English artist aliasing.

use super::table::TRADITIONAL_TO_SIMPLIFIED;
use crate::text;
use crate::track::TrackName;

/// Converts all Traditional Chinese characters in `text` to Simplified Chinese.
pub fn to_simplified(text: &str) -> String {
    text.chars()
        .map(|c| {
            match TRADITIONAL_TO_SIMPLIFIED
                .binary_search_by_key(&c, |&(traditional, _)| traditional)
            {
                Ok(index) => {
                    let (_, simplified) = TRADITIONAL_TO_SIMPLIFIED[index];
                    simplified
                }
                Err(_) => c,
            }
        })
        .collect()
}

/// Known English and romanized artist names mapped to Simplified Chinese.
const ARTIST_ALIASES: &[(&str, &str)] = &[
    ("a-mei", "张惠妹"),
    ("amei", "张惠妹"),
    ("alan tam", "谭咏麟"),
    ("andy lau", "刘德华"),
    ("angela chang", "张韶涵"),
    ("anita mui", "梅艳芳"),
    ("charlie zhou", "周深"),
    ("crowd lu", "卢广仲"),
    ("david tao", "陶喆"),
    ("eason chan", "陈奕迅"),
    ("eric chou", "周兴哲"),
    ("faye wong", "王菲"),
    ("fish leong", "梁静茹"),
    ("g.e.m.", "邓紫棋"),
    ("gem", "邓紫棋"),
    ("george lam", "林子祥"),
    ("gigi leung", "梁咏琪"),
    ("hacken lee", "李克勤"),
    ("hebe tien", "田馥甄"),
    ("jacky cheung", "张学友"),
    ("jay chou", "周杰伦"),
    ("jj lin", "林俊杰"),
    ("joker xue", "薛之谦"),
    ("jolin tsai", "蔡依林"),
    ("karen mok", "莫文蔚"),
    ("kelly chen", "陈慧琳"),
    ("leehom wang", "王力宏"),
    ("leo ku", "古巨基"),
    ("leslie cheung", "张国荣"),
    ("li ronghao", "李荣浩"),
    ("mayday", "五月天"),
    ("miriam yeung", "杨千嬅"),
    ("sammi cheng", "郑秀文"),
    ("stefanie sun", "孙燕姿"),
    ("teresa teng", "邓丽君"),
    ("wang leehom", "王力宏"),
    ("yoga lin", "林宥嘉"),
    ("zhou shen", "周深"),
];

/// Known romanized/Pinyin song title aliases mapped to Simplified Chinese.
const TITLE_ALIASES: &[(&str, &str)] = &[
    ("ai ru chao shui", "爱如潮水"),
    ("an jing", "安静"),
    ("bu zai you yu", "不再犹豫"),
    ("cang hai yi sheng xiao", "沧海一声笑"),
    ("cheng du", "成都"),
    ("da di", "大地"),
    ("da yu", "大鱼"),
    ("dan shen qing ge", "单身情歌"),
    ("dao xiang", "稻香"),
    ("feng", "枫"),
    ("fu shi shan xia", "富士山下"),
    ("gu yong zhe", "孤勇者"),
    ("guang hui sui yue", "光辉岁月"),
    ("hai kuo tian kong", "海阔天空"),
    ("hong ri", "红日"),
    ("hua hai", "花海"),
    ("hui se gui ji", "灰色轨迹"),
    ("kua fu", "夸父"),
    ("leng yu ye", "冷雨夜"),
    ("li bai", "李白"),
    ("mo li hua", "茉莉花"),
    ("nan ban qiu de xue", "南半球的雪"),
    ("nian shao you wei", "年少有为"),
    ("peng you", "朋友"),
    ("ping fan zhi lu", "平凡之路"),
    ("qi feng le", "起风了"),
    ("qi li xiang", "七里香"),
    ("qian li zhi wai", "千里之外"),
    ("qian qian que ge", "千千阙歌"),
    ("qing hua ci", "青花瓷"),
    ("qing ren", "情人"),
    ("qing tian", "晴天"),
    ("re qing de sha mo", "热情的沙漠"),
    ("ru yuan", "如愿"),
    ("shi nian", "十年"),
    ("sui yue wu sheng", "岁月无声"),
    ("tian mi mi", "甜蜜蜜"),
    ("tong hua", "童话"),
    ("wan an", "晚安"),
    ("wen bie", "吻别"),
    ("wo shi ge xu yao hen duo ai de ren", "我是个需要很多爱的人"),
    ("wu hui zhe yi sheng", "无悔这一生"),
    ("xi huan ni", "喜欢你"),
    ("xiao chou", "消愁"),
    ("xin tai ruan", "心太软"),
    ("ye qu", "夜曲"),
    ("yue ban xiao ye qu", "月半小夜曲"),
    ("yue liang dai biao wo de xin", "月亮代表我的心"),
    ("zao an", "早安"),
    ("zhen de ai ni", "真的爱你"),
];

fn artist_alias(artist: &str) -> Option<&'static str> {
    let lower = artist.trim().to_lowercase();
    ARTIST_ALIASES
        .iter()
        .find(|&&(alias, _)| lower == alias)
        .map(|&(_, chinese)| chinese)
}

/// The Chinese title for a romanized one such as "Hui Se Gui Ji (Movie Theme)", ignoring
/// bracketed extras, case and punctuation.
fn title_alias(title: &str) -> Option<&'static str> {
    let cleaned = text::normalize_text(&text::strip_brackets(title, |_| true));
    if cleaned.is_empty() {
        return None;
    }
    TITLE_ALIASES
        .iter()
        .find(|&&(alias, _)| {
            // The whole title, or its last words ("Beyond Hui Se Gui Ji")
            cleaned
                .strip_suffix(alias)
                .is_some_and(|rest| rest.is_empty() || rest.ends_with(' '))
        })
        .map(|&(_, chinese)| chinese)
}

/// A song's title and artist in Simplified Chinese, with known romanized names replaced by
/// their Chinese ones. Names with Japanese kana or Korean hangul are returned unchanged.
pub fn simplified_name(title: &str, artist: &str) -> TrackName {
    if text::has_kana_or_hangul(title) || text::has_kana_or_hangul(artist) {
        return TrackName {
            title: title.to_string(),
            artist: artist.to_string(),
        };
    }
    TrackName {
        title: title_alias(title).map_or_else(|| to_simplified(title), str::to_string),
        artist: artist_alias(artist).map_or_else(|| to_simplified(artist), str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_is_sorted() {
        for window in TRADITIONAL_TO_SIMPLIFIED.windows(2) {
            assert!(
                window[0].0 < window[1].0,
                "Table not sorted: {:?} vs {:?}",
                window[0],
                window[1]
            );
        }
    }

    #[test]
    fn test_to_simplified_conversion() {
        assert_eq!(to_simplified("周杰倫 - 晴天"), "周杰伦 - 晴天");
        assert_eq!(to_simplified("陳奕迅 - 十年"), "陈奕迅 - 十年");
        assert_eq!(to_simplified("Beyond - 海闊天空"), "Beyond - 海阔天空");
        assert_eq!(to_simplified("張學友 - 吻別"), "张学友 - 吻别");
        assert_eq!(
            to_simplified("今天我 寒夜裏看雪飄過 懷着冷卻了的心窩飄遠方"),
            "今天我 寒夜里看雪飘过 怀着冷却了的心窝飘远方"
        );
    }

    #[test]
    fn test_simplified_name_resolves_aliases_and_leaves_japanese_alone() {
        let name = |title: &str, artist: &str| TrackName {
            title: title.to_string(),
            artist: artist.to_string(),
        };
        assert_eq!(simplified_name("晴天", "Jay Chou"), name("晴天", "周杰伦"));
        assert_eq!(
            simplified_name("海闊天空", "Beyond"),
            name("海阔天空", "Beyond")
        );
        assert_eq!(
            simplified_name(
                "Hui Se Gui Ji (Dian Ying \"Tian Ruo You Qing\" Ge Qu)",
                "Beyond"
            ),
            name("灰色轨迹", "Beyond")
        );
        assert_eq!(
            simplified_name("Shape of You", "Ed Sheeran"),
            name("Shape of You", "Ed Sheeran")
        );
        // 愛 is also Japanese; kana in the title mark the song as Japanese
        assert_eq!(
            simplified_name("愛にできることはまだあるかい", "RADWIMPS"),
            name("愛にできることはまだあるかい", "RADWIMPS")
        );
    }
}
