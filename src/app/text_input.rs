//! A line of text being typed, with a cursor that `←` and `→` move along it.

/// The text of a station field or the search prompt while it is being typed.
#[derive(Debug, Default)]
pub struct TextInput {
    text: String,
    /// Byte index into `text` where typing inserts, always on a character boundary.
    cursor: usize,
}

impl TextInput {
    /// Starts with the cursor after the last character.
    pub fn new(text: String) -> Self {
        Self {
            cursor: text.len(),
            text,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn before_cursor(&self) -> &str {
        &self.text[..self.cursor]
    }

    /// The text from the cursor on, so its first character is the one under the cursor.
    pub fn after_cursor(&self) -> &str {
        &self.text[self.cursor..]
    }

    pub fn insert(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    /// Removes the character left of the cursor, if there is one.
    pub fn backspace(&mut self) {
        if let Some(character) = self.before_cursor().chars().next_back() {
            self.cursor -= character.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    /// Removes the character under the cursor, if there is one.
    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    /// Moves one character left; stays put at the start.
    pub fn move_left(&mut self) {
        if let Some(character) = self.before_cursor().chars().next_back() {
            self.cursor -= character.len_utf8();
        }
    }

    /// Moves one character right; stays put after the last character.
    pub fn move_right(&mut self) {
        if let Some(character) = self.after_cursor().chars().next() {
            self.cursor += character.len_utf8();
        }
    }

    /// Moves the cursor to the start of the text.
    pub fn move_start(&mut self) {
        self.cursor = 0;
    }

    /// Moves the cursor past the last character.
    pub fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_typing_and_backspace_act_at_the_cursor() {
        let mut input = TextInput::new("Jazz".to_string());
        input.insert('!');
        assert_eq!(input.as_str(), "Jazz!");

        input.move_left();
        input.move_left();
        input.insert('y');
        assert_eq!(input.as_str(), "Jazyz!");
        assert_eq!(input.before_cursor(), "Jazy");
        assert_eq!(input.after_cursor(), "z!");

        input.backspace();
        input.backspace();
        assert_eq!(input.as_str(), "Jaz!");
        assert_eq!(input.after_cursor(), "z!");
    }

    #[test]
    fn test_cursor_stops_at_both_ends() {
        let mut input = TextInput::new("ab".to_string());
        input.move_right();
        assert_eq!(input.after_cursor(), "");

        for _ in 0..3 {
            input.move_left();
        }
        assert_eq!(input.before_cursor(), "");
        input.backspace();
        assert_eq!(input.as_str(), "ab");

        input.clear();
        input.insert('c');
        assert_eq!(input.as_str(), "c");
    }

    #[test]
    fn test_cursor_moves_by_whole_characters() {
        // Chinese characters take three bytes each in UTF-8
        let mut input = TextInput::new("古典音乐".to_string());
        input.move_left();
        assert_eq!(input.after_cursor(), "乐");
        input.backspace();
        assert_eq!(input.as_str(), "古典乐");
        input.move_right();
        input.insert('台');
        assert_eq!(input.as_str(), "古典乐台");

        input.move_start();
        assert_eq!(input.before_cursor(), "");
        input.delete();
        assert_eq!(input.as_str(), "典乐台");
        input.move_end();
        input.delete();
        assert_eq!(input.as_str(), "典乐台");
    }
}
