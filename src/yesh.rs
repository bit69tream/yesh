use gettextrs::{setlocale, LocaleCategory};
use ncursesw::*;
use std::panic::PanicInfo;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct LineView {
    y: i32,
    width: i32,
    index: usize,
    offset: usize,
}

struct CommandView {
    y: i32,
    width: i32,
    offset: usize,
}

pub struct Yesh<'a> {
    window_size: Size,
    window: WINDOW,

    attributes: normal::Attributes,
    color_pair: normal::ColorPair,

    prompt: &'a str,
    command: Vec<WideChar>,
    command_views: Vec<CommandView>,

    lines: Vec<Vec<ComplexChar>>,
    line_views: Vec<LineView>,
    cursor_position: Origin,
    scroll_offset: i32,

    control_c_semaphore: Arc<AtomicBool>,

    should_exit: bool,

    running_command: Option<Child>,
}

impl Yesh<'_> {
    pub fn new() -> Result<Self, ncursesw::NCurseswError> {
        let control_c_semaphore: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        let control_c_semaphore_clone = Arc::clone(&control_c_semaphore);
        ctrlc::set_handler(move || {
            control_c_semaphore_clone.store(true, Ordering::Relaxed);
        })
        .expect("cannot set control-c handler");

        setlocale(LocaleCategory::LcAll, "");
        let window = initscr()?;
        cbreak()?;
        noecho()?;
        keypad(window, true)?;
        // NOTE: the api here is just dumb.
        //       they accept `Duration`, then they take it as seconds and divide it by 10.
        //       and then that value is passen to ncurses library.
        halfdelay(Duration::from_secs(10))?;

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

            attributes,
            color_pair,

            prompt,
            command: Vec::new(),
            command_views: Vec::new(),

            lines: Vec::new(),
            line_views: Vec::new(),
            cursor_position: Origin { x: prompt.len() as i32, y: 0 },
            scroll_offset: 0,

            control_c_semaphore,

            should_exit: false,

            running_command: None,
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

                if self.command.len() == 0 {
                    self.should_exit = true;
                } else if index < self.command.len() {
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


    fn execute_command(&mut self) -> Result<(), NCurseswError> {
        // let command = self.into_command();

        let mut prompt_line: Vec<ComplexChar> = Vec::new();

        for character in self.prompt.chars() {
            prompt_line.push(ComplexChar::from_char(character, &self.attributes, &self.color_pair)?);
        }

        for character in &self.command {
            prompt_line.push(ComplexChar::from_wide_char(*character, &self.attributes, &self.color_pair)?);
        }
        self.command.clear();

        self.cursor_position.y += 1;

        self.lines.push(prompt_line);

        Ok(())
    }

    fn process_control_character(&mut self, control_character: char) {
        use ascii::{AsciiChar, ToAsciiChar};

        match control_character.to_ascii_char().unwrap() {
            AsciiChar::LineFeed => {
                self.execute_command().unwrap();
            }

            // NOTE: control-c
            AsciiChar::ETX => {}

            // NOTE: control-d
            AsciiChar::EOT => {
                let index = self.cursor_position.x as usize - self.prompt.len();

                if self.command.len() == 0 {
                    self.should_exit = true;
                } else if index < self.command.len() {
                    self.command.remove(index);
                }
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
                self.process_control_character(control_character);
                return Ok(());
            }
        }

        self.command.push(character);
        self.cursor_position.x += 1;

        Ok(())
    }

    fn clamp_cursor(&mut self) {
        self.cursor_position.x = self.cursor_position.x.clamp(self.prompt.len() as i32, (self.command.len() + self.prompt.len()) as i32);
        self.cursor_position.y = self.cursor_position.y.clamp(0, self.window_size.lines as i32);
    }

    pub fn process_events(&mut self) -> Result<bool, ncursesw::NCurseswError> {
        use ncursesw::CharacterResult::{Character, Key};

        if let Ok(key_or_character) = wget_wch(self.window) {
            match key_or_character {
                Key(key) => self.process_key(key)?,
                Character(character) => self.process_character(character)?,
            }
        } else if self.semaphore.load(Ordering::Relaxed) {
            use ascii::AsciiChar;

            self.process_control_character(AsciiChar::ETX.as_char());
        }

        self.clamp_cursor();

        return Ok(self.should_exit);
    }

    pub fn render(&self) -> Result<(), ncursesw::NCurseswError> {
        use ascii::AsciiChar;

        wclear(self.window)?;

        wmove(self.window, Origin::default())?;
        for line in &self.lines {
            for character in line {
                wadd_wch(self.window, *character)?;
            }
            waddch(self.window, ChtypeChar::new(AsciiChar::LineFeed))?;
        }

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
