//! Android selections use UTF-16 offsets; egui selections use Unicode character offsets.
pub fn utf16_to_chars(text: &str, offset: usize) -> usize {
    let mut units = 0;
    text.chars()
        .take_while(|ch| {
            units += ch.len_utf16();
            units <= offset
        })
        .count()
}

pub fn chars_to_utf16(text: &str, offset: usize) -> usize {
    text.chars().take(offset).map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn android_selection_handles_accents_and_surrogate_pairs() {
        let text = "É🚗 Clio";
        assert_eq!(chars_to_utf16(text, 2), 3);
        assert_eq!(utf16_to_chars(text, 3), 2);
        assert_eq!(utf16_to_chars(text, 2), 1);
        for i in 0..=text.chars().count() {
            assert_eq!(utf16_to_chars(text, chars_to_utf16(text, i)), i);
        }
        assert_eq!(utf16_to_chars(text, usize::MAX), text.chars().count());
    }
}
