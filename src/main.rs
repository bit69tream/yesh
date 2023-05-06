use gettextrs::{setlocale, LocaleCategory};
use ncursesw::*;

struct Yesh {
    size: Size,
    window: WINDOW,
}

impl Yesh {
    fn new() -> Result<Self, ncursesw::NCurseswError> {
        // TODO: also handle ctrl-d (EINTR)
        ctrlc::set_handler(move || {}).expect("cannot set ctrl-c handler");

        setlocale(LocaleCategory::LcAll, "");
        let window = initscr()?;
        cbreak()?;
        noecho()?;
        keypad(window, true)?;

        let yesh = Yesh { window, size: getmaxyx(window)? };
        Ok(yesh)
    }

    fn process_key(&mut self, key: KeyBinding) -> Result<(), ncursesw::NCurseswError> {
        Ok(())
    }

    fn process_character(&mut self, character: char) -> Result<(), ncursesw::NCurseswError> {
        Ok(())
    }

    fn process_events(&mut self) -> Result<bool, ncursesw::NCurseswError> {
        use ncursesw::CharacterResult::{Character, Key};

        let key_or_character = wgetch(self.window)?;
        match key_or_character {
            Key(key) => self.process_key(key)?,
            Character(character) => self.process_character(character)?,
        }

        return Ok(false);
    }

    fn render(&self) -> Result<(), ncursesw::NCurseswError> {
        wrefresh(self.window)
    }
}

impl Drop for Yesh {
    fn drop(&mut self) {
        endwin().unwrap();
    }
}

fn real_main() -> Result<(), NCurseswError> {
    let mut yesh = Yesh::new()?;
    while !yesh.process_events()? {
        yesh.render()?;
    }

    Ok(())
}

fn main() {
    if let Err(error) = real_main() {
        panic!("{}", error.to_string());
    }
}
