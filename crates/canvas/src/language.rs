/// Windows LCIDs keyed by BCP-47 language, `language-Script` or `language-REGION`; a bare
/// language maps to the locale Windows picks for it.
const LCIDS: &[(&str, u32)] = &[
    ("af", 1078),
    ("am", 1118),
    ("ar", 1025),
    ("ar-EG", 3073),
    ("az", 1068),
    ("be", 1059),
    ("bg", 1026),
    ("bn", 1093),
    ("bn-BD", 2117),
    ("ca", 1027),
    ("cs", 1029),
    ("cy", 1106),
    ("da", 1030),
    ("de", 1031),
    ("de-AT", 3079),
    ("de-CH", 2055),
    ("de-LI", 5127),
    ("de-LU", 4103),
    ("el", 1032),
    ("en", 1033),
    ("en-AU", 3081),
    ("en-CA", 4105),
    ("en-GB", 2057),
    ("en-IE", 6153),
    ("en-IN", 16393),
    ("en-NZ", 5129),
    ("en-SG", 18441),
    ("en-ZA", 7177),
    ("es", 3082),
    ("es-MX", 2058),
    ("es-US", 21514),
    ("et", 1061),
    ("eu", 1069),
    ("fa", 1065),
    ("fi", 1035),
    ("fil", 1124),
    ("fr", 1036),
    ("fr-BE", 2060),
    ("fr-CA", 3084),
    ("fr-CH", 4108),
    ("gl", 1110),
    ("gu", 1095),
    ("he", 1037),
    ("hi", 1081),
    ("hr", 1050),
    ("hu", 1038),
    ("hy", 1067),
    ("id", 1057),
    ("is", 1039),
    ("it", 1040),
    ("it-CH", 2064),
    ("ja", 1041),
    ("ka", 1079),
    ("kk", 1087),
    ("km", 1107),
    ("kn", 1099),
    ("ko", 1042),
    ("lo", 1108),
    ("lt", 1063),
    ("lv", 1062),
    ("mk", 1071),
    ("ml", 1100),
    ("mn", 1104),
    ("mr", 1102),
    ("ms", 1086),
    ("my", 1109),
    ("nb", 1044),
    ("ne", 1121),
    ("nl", 1043),
    ("nl-BE", 2067),
    ("nn", 2068),
    ("no", 1044),
    ("pa", 1094),
    ("pl", 1045),
    ("pt", 1046),
    ("pt-PT", 2070),
    ("ro", 1048),
    ("ru", 1049),
    ("si", 1115),
    ("sk", 1051),
    ("sl", 1060),
    ("sq", 1052),
    ("sr", 10266),
    ("sr-Latn", 9242),
    ("sv", 1053),
    ("sv-FI", 2077),
    ("sw", 1089),
    ("ta", 1097),
    ("te", 1098),
    ("th", 1054),
    ("tr", 1055),
    ("uk", 1058),
    ("ur", 1056),
    ("vi", 1066),
    ("zh", 2052),
    ("zh-HK", 3076),
    ("zh-Hant", 1028),
    ("zh-MO", 5124),
    ("zh-SG", 4100),
    ("zh-TW", 1028),
];

/// en-US, what OneNote records when nothing better is known.
pub(crate) const EN_US: u32 = 1033;

/// The Windows LCID of a BCP-47 tag such as an input source's language (`de`, `en-GB`,
/// `zh-Hans`), matched by region, then script, then language, else en-US.
pub fn lcid(tag: &str) -> u32 {
    let mut subtags = tag.split(['-', '_']);
    let language = subtags.next().unwrap_or_default();
    let (mut script, mut region) = (None, None);
    for subtag in subtags {
        match subtag.len() {
            4 => script = Some(subtag),
            2 | 3 => region = Some(subtag),
            _ => {}
        }
    }
    [region, script]
        .into_iter()
        .flatten()
        .map(|subtag| format!("{language}-{subtag}"))
        .chain([language.to_owned()])
        .find_map(|key| {
            LCIDS
                .iter()
                .find(|(tag, _)| tag.eq_ignore_ascii_case(&key))
                .map(|(_, lcid)| *lcid)
        })
        .unwrap_or(EN_US)
}

/// The BCP-47 tag of a Windows LCID; a bare language stands for the locale Windows picks
/// for it. None for LCIDs outside the table, such as math's.
pub fn tag(lcid: u32) -> Option<&'static str> {
    LCIDS
        .iter()
        .find(|(_, known)| *known == lcid)
        .map(|(tag, _)| *tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_map_to_windows_lcids() {
        for (tag, lcid) in [
            ("en", 1033),
            ("en-US", 1033),
            ("en_GB", 2057),
            ("de", 1031),
            ("DE-at", 3079),
            ("fr-FR", 1036),
            ("es-419", 3082),
            ("ja-JP", 1041),
            ("zh-Hans", 2052),
            ("zh-Hant", 1028),
            ("zh-Hant-HK", 3076),
            ("zh-Hans-SG", 4100),
            ("sr-Latn-RS", 9242),
            ("emoji", EN_US),
            ("", EN_US),
        ] {
            assert_eq!(super::lcid(tag), lcid, "{tag}");
        }
        for (lcid, tag) in [(1033, Some("en")), (2057, Some("en-GB")), (0x1007f, None)] {
            assert_eq!(super::tag(lcid), tag);
        }
    }
}
