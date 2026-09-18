use crate::config::profile::IconSet;

#[derive(Debug, PartialEq, Eq)]
pub struct Icons {
    pub refresh: &'static str,
    pub arrow: &'static str,
    pub enter: &'static str,
    pub esc: &'static str,
}

const NERD: Icons = Icons {
    refresh: "\u{f021}",
    arrow: "\u{f061}",
    enter: "\u{f0311}",
    esc: "\u{f12b7}",
};

const UNICODE: Icons = Icons {
    refresh: "↻",
    arrow: "→",
    enter: "↵",
    esc: "Esc",
};

pub fn icons(set: IconSet) -> &'static Icons {
    match set {
        IconSet::Nerd => &NERD,
        IconSet::Unicode => &UNICODE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_set_avoids_private_use_glyphs() {
        let private_use = |c: char| matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0x10FFFF);
        let unicode = icons(IconSet::Unicode);
        for icon in [unicode.refresh, unicode.arrow, unicode.enter, unicode.esc] {
            assert!(!icon.chars().any(private_use), "{icon:?}");
        }
        let nerd = icons(IconSet::Nerd);
        for icon in [nerd.refresh, nerd.arrow, nerd.enter, nerd.esc] {
            assert!(icon.chars().all(private_use), "{icon:?}");
        }
    }
}
