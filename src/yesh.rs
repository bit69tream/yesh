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
        //       the value is accepted as a `Duration`, then gets taken as seconds and divided by 10
        halfdelay(Duration::from_secs(1 * 10))?;

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

    fn handle_resize(&mut self) -> Result<(), ncursesw::NCurseswError> {
        self.window_size = getmaxyx(self.window)?;
        self.rebuild_line_views();
        Ok(())
    }

    fn rebuild_line_views(&mut self) {
        self.line_views.clear();

        let mut y = 0i32;
        for (index, line) in self.lines.iter().enumerate() {
            let mut remaining_width = line.len() as i32;
            let mut offset: usize = 0;

            loop {
                let view_width = self.window_size.columns.min(remaining_width);
                self.line_views.push(LineView { y, width: view_width, index, offset });

                y += 1;
                offset += (view_width - 1) as usize;
                remaining_width -= view_width;

                if remaining_width <= 0 {
                    break;
                }
            }
        }
    }

    fn rebuild_command_views(&mut self) {
        self.command_views.clear();

        let mut y: i32 = if self.line_views.len() > 0 { self.line_views[self.line_views.len() - 1].y + 1 } else { 0 };
        let mut remaining_width: i32 = self.command.len() as i32;
        let mut offset: usize = 0;

        loop {
            let max_view_width = if self.command_views.len() == 0 {
                self.window_size.columns - self.prompt.len() as i32
            } else {
                self.window_size.columns
            };
            let view_width = remaining_width.min(max_view_width);

            self.command_views.push(CommandView { y, width: view_width, offset });

            y += 1;
            offset += (view_width - 1) as usize;
            remaining_width -= view_width;

            if remaining_width <= 0 {
                break;
            }
        }
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

            UpArrow => {
                self.cursor_position.y -= 1;
            }

            DownArrow => {
                self.cursor_position.y += 1;
            }

            ResizeEvent => {
                self.handle_resize()?;
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
        self.command_views.clear();

        self.cursor_position.y += 1;
        self.cursor_position.x = self.prompt.len() as i32;

        self.lines.push(prompt_line);
        self.rebuild_line_views();

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
                self.advance_cursor_left();
            }
            _ => {}
        }
    }

    fn advance_cursor_left(&mut self) {
        self.cursor_position.x -= 1;
    }

    fn advance_cursor_right(&mut self) {
        self.cursor_position.x += 1;
        if self.cursor_position.x >= self.window_size.columns as i32 {
            self.cursor_position.y += 1;
            self.cursor_position.x = 0;
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
        self.advance_cursor_right();
        self.rebuild_command_views();

        Ok(())
    }

    fn clamp_cursor(&mut self) {
        let maximum_allowed_x = (self.command.len() + self.prompt.len()).min((self.window_size.columns - 1) as usize);

        self.cursor_position.x = self.cursor_position.x.clamp(0, maximum_allowed_x as i32);
        self.cursor_position.y = self.cursor_position.y.clamp(0, self.window_size.lines as i32);
    }

    pub fn process_events(&mut self) -> Result<bool, ncursesw::NCurseswError> {
        use ncursesw::CharacterResult::{Character, Key};

        if let Ok(key_or_character) = wget_wch(self.window) {
            match key_or_character {
                Key(key) => self.process_key(key)?,
                Character(character) => self.process_character(character)?,
            }
        } else if self.control_c_semaphore.load(Ordering::Relaxed) {
            use ascii::AsciiChar;

            self.process_control_character(AsciiChar::ETX.as_char());
        }

        self.clamp_cursor();

        return Ok(self.should_exit);
    }

    fn is_y_on_screen(&self, y: i32) -> bool {
        y >= self.scroll_offset && y < (self.scroll_offset + self.window_size.lines)
    }

    fn render_lines(&self) -> Result<i32, ncursesw::NCurseswError> {
        let mut y = 0;
        let mut drawn_something = false;
        for view in &self.line_views {
            y = view.y - self.scroll_offset;
            if !self.is_y_on_screen(view.y) {
                continue;
            }

            drawn_something = true;
            wmove(self.window, Origin { x: 0, y })?;
            for i in 0..view.width as usize {
                wadd_wch(self.window, self.lines[view.index][view.offset + i])?;
            }
        }

        if drawn_something {
            y += 1;
        }

        Ok(y)
    }

    fn render_command(&self, prompt_y: i32) -> Result<(), ncursesw::NCurseswError> {
        if self.running_command.is_some() {
            return Ok(());
        }

        if !self.is_y_on_screen(prompt_y) {
            return Ok(());
        }

        wmove(self.window, Origin { x: 0, y: prompt_y })?;
        waddstr(self.window, self.prompt)?;

        let mut first_line: bool = true;
        for view in &self.command_views {
            let y = view.y - self.scroll_offset;

            wmove(
                self.window,
                Origin {
                    x: if first_line { self.prompt.len() as i32 } else { 0 },
                    y,
                },
            )?;
            first_line = false;
            if !self.is_y_on_screen(y) {
                continue;
            }

            for i in 0..view.width as usize {
                wadd_wch(self.window, ComplexChar::from_wide_char(self.command[view.offset + i], &self.attributes, &self.color_pair)?)?;
            }
        }

        Ok(())
    }

    pub fn render(&self) -> Result<(), ncursesw::NCurseswError> {
        wclear(self.window)?;

        let prompt_y = self.render_lines()?;
        self.render_command(prompt_y)?;

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
