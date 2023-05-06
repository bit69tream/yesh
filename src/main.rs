use gettextrs::{setlocale, LocaleCategory};
use ncursesw::*;
use std::panic;

struct Yesh {
    size: Size,
    window: WINDOW,
    command: Vec<WideChar>,
    attributes: normal::Attributes,
    color_pair: normal::ColorPair,
}

impl Yesh {
    fn new() -> Result<Self, ncursesw::NCurseswError> {
        // TODO: also handle ctrl-d (EINTR)
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

        let yesh = Yesh {
            window,
            size: getmaxyx(window)?,
            command: Vec::new(),
            attributes,
            color_pair,
        };
        Ok(yesh)
    }

    fn process_key(&mut self, key: KeyBinding) -> Result<(), ncursesw::NCurseswError> {
        Ok(())
    }

    fn process_character(&mut self, character: WideChar) -> Result<(), ncursesw::NCurseswError> {
        if let Ok(value) = character.as_char() {
            if value.is_control() {
                return Ok(());
            }
        }
        self.command.push(character);

        // wadd_wch(self.window, ComplexChar::from_wide_char(character, &self.attributes, &self.color_pair)?)?;
        Ok(())
    }

    fn process_events(&mut self) -> Result<bool, ncursesw::NCurseswError> {
        use ncursesw::CharacterResult::{Character, Key};

        let key_or_character = wget_wch(self.window)?;
        match key_or_character {
            Key(key) => self.process_key(key)?,
            Character(character) => self.process_character(character)?,
        }

        return Ok(false);
    }

    fn render(&self) -> Result<(), ncursesw::NCurseswError> {
        wclear(self.window);

        wmove(self.window, Origin::default())?;
        waddstr(self.window, "%> ");
        for character in &self.command {
            wadd_wch(self.window, ComplexChar::from_wide_char(*character, &self.attributes, &self.color_pair)?)?;
        }

        wrefresh(self.window)
    }
}

impl Drop for Yesh {
    fn drop(&mut self) {
        close_ncurses_window();
    }
}

fn real_main() -> Result<(), NCurseswError> {
    let mut yesh = Yesh::new()?;
    while !yesh.process_events()? {
        yesh.render()?;
    }

    Ok(())
}

fn close_ncurses_window() {
    if !isendwin() {
        endwin().unwrap()
    }
}

fn main() {
    panic::set_hook(Box::new(|panic_info| {
        // NOTE: nothing will be printed if ncurses window is still open
        close_ncurses_window();
        eprintln!("{}", panic_info);
    }));

    if let Err(error) = real_main() {
        panic!("{}", error.to_string());
    }
}
