use std::panic::PanicInfo;
use gettextrs::{setlocale, LocaleCategory};
use ncursesw::*;

pub struct Yesh<'a> {
    window_size: Size,
    window: WINDOW,
    command: Vec<WideChar>,
    attributes: normal::Attributes,
    color_pair: normal::ColorPair,
    prompt: &'a str,
    cursor_position: Origin,
}

impl Yesh<'_> {
    pub fn new() -> Result<Self, ncursesw::NCurseswError> {
        ctrlc::set_handler(|| {}).expect("cannot set ctrl-c handler");

        setlocale(LocaleCategory::LcAll, "");
        let window = initscr()?;
        cbreak()?;
        noecho()?;
        keypad(window, true)?;

        let (attributes, color_pair) = {
            use ncursesw::AttributesColorPairSet::{Extend, Normal};
            match wattr_get(window)? {
                Normal(attributes_and_color_pair) => (attributes_and_color_pair.attributes(), attributes_and_color_pair.color_pair()),
                Extend(_) => panic!("extended attributes and color pairs are not supported"),
            }
        };

        let prompt = "% ";
        let yesh = Yesh {
            window,
            window_size: getmaxyx(window)?,
            command: Vec::new(),
            attributes,
            color_pair,
            prompt,
            cursor_position: Origin { x: prompt.len() as i32, y: 0 },
        };
        Ok(yesh)
    }

    fn process_key(&mut self, key: KeyBinding) -> Result<(), ncursesw::NCurseswError> {
        use ncursesw::KeyBinding::*;

        match key {
            Backspace => {
                self.cursor_position.x -= 1;
                self.command.pop();
            }
            DeleteCharacter => {
                let index = self.cursor_position.x as usize - self.prompt.len();

                if index < self.command.len() {
                    self.command.remove(index);
                }
            }
            LeftArrow => {
                self.cursor_position.x -= 1;
            }
            RightArrow => {
                self.cursor_position.x += 1;
            }
            _ => {}
        }

        Ok(())
    }

    fn execute_command(&mut self) {
        todo!()
    }

    fn process_control_characters(&mut self, control_character: char) {
        use ascii::{AsciiChar, ToAsciiChar};

        match control_character.to_ascii_char().unwrap() {
            AsciiChar::LineFeed => {
                self.execute_command();
            }

            // NOTE: for some reason pressing backspace produces DEL. actual delete key is processed in `process_key`
            AsciiChar::BackSpace | AsciiChar::DEL => {
                self.command.pop();
                self.cursor_position.x -= 1;
            }
            _ => {}
        }
    }

    fn process_character(&mut self, character: WideChar) -> Result<(), ncursesw::NCurseswError> {
        if let Ok(control_character) = character.as_char() {
            if control_character.is_control() {
                self.process_control_characters(control_character);
                return Ok(());
            }
        }

        self.command.push(character);
        self.cursor_position.x += 1;

        Ok(())
    }

    fn clamp_cursor(&mut self) {
        self.cursor_position.x = self.cursor_position.x.clamp(self.prompt.len() as i32,
                                                              (self.command.len() + self.prompt.len()) as i32);
        self.cursor_position.y = self.cursor_position.y.clamp(0, self.window_size.lines as i32);
    }

    pub fn process_events(&mut self) -> Result<bool, ncursesw::NCurseswError> {
        use ncursesw::CharacterResult::{Character, Key};

        let key_or_character = wget_wch(self.window)?;
        match key_or_character {
            Key(key) => self.process_key(key)?,
            Character(character) => self.process_character(character)?,
        }

        self.clamp_cursor();

        return Ok(false);
    }

    pub fn render(&self) -> Result<(), ncursesw::NCurseswError> {
        wclear(self.window)?;

        wmove(self.window, Origin::default())?;
        waddstr(self.window, self.prompt)?;
        for character in &self.command {
            wadd_wch(self.window, ComplexChar::from_wide_char(*character, &self.attributes, &self.color_pair)?)?;
        }

        wmove(self.window, self.cursor_position)?;

        wrefresh(self.window)
    }
}

impl Drop for Yesh<'_> {
    fn drop(&mut self) {
        close_ncurses_window();
    }
}

fn close_ncurses_window() {
    if !isendwin() {
        endwin().unwrap()
    }
}

pub fn panic_hook(info: &PanicInfo<'_>) {
    // NOTE: nothing will be printed if ncurses window is still open
    close_ncurses_window();
    eprintln!("{}", info);
}
