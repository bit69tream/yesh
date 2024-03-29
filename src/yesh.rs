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

    running_child: Option<Child>,
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

            running_child: None,
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
                offset += view_width as usize;
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
            offset += view_width as usize;
            remaining_width -= view_width;

            if remaining_width <= 0 {
                break;
            }
        }
    }

    fn delete_character_before_cursor(&mut self) {
        if let Some(index) = self.command_index_at_cursor() {
            if index == 0 {
                return;
            }

            self.command.remove(index - 1);
            self.rebuild_command_views();
            self.advance_cursor_left();
        }
    }

    fn delete_character_at_cursor(&mut self) {
        if let Some(index) = self.command_index_at_cursor() {
            if index == 0 {
                return;
            }

            self.command.remove(index);
            self.rebuild_command_views();
        }
    }

    fn process_key(&mut self, key: KeyBinding) -> Result<(), ncursesw::NCurseswError> {
        use ncursesw::KeyBinding::*;

        match key {
            Backspace => self.delete_character_before_cursor(),
            DeleteCharacter => self.delete_character_at_cursor(),
            LeftArrow => self.advance_cursor_left(),
            RightArrow => self.advance_cursor_right(),
            UpArrow => self.advance_cursor_up(),
            DownArrow => self.advance_cursor_down(),
            ResizeEvent => self.handle_resize()?,
            _ => {}
        }

        Ok(())
    }

    fn execute_command(&mut self) -> Result<(), NCurseswError> {
        let mut prompt_line: Vec<ComplexChar> = Vec::new();

        if !self.is_cursor_on_command_prompt() {
            return Ok(());
        }

        for character in self.prompt.chars() {
            prompt_line.push(ComplexChar::from_char(character, &self.attributes, &self.color_pair)?);
        }

        for character in &self.command {
            prompt_line.push(ComplexChar::from_wide_char(*character, &self.attributes, &self.color_pair)?);
        }

        self.lines.push(prompt_line);

        let parsed_command = parse_command(&self.command);

        self.command.clear();
        self.command_views.clear();

        if parsed_command.len() > 0 {
            if parsed_command[0] == "info" {
                let info_message = r#"    yesh  Copyright (C) 2023 bit69tream
    This program comes with ABSOLUTELY NO WARRANTY;
    This is free software, and you are welcome to redistribute it under certain conditions;
    See <https://www.gnu.org/licenses/>"#;
                self.append_to_lines(info_message)?;
            } else if parsed_command[0] == "exit" {
                self.should_exit = true;
            } else {
                let child = Command::new(&parsed_command[0])
                    .args(&parsed_command[1..])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();

                match child {
                    Ok(successful_child) => self.running_child = Some(successful_child),
                    Err(failed_child) => {
                        let error_message: String = "yesh: ERROR: Failed to launch command: ".to_string() + &failed_child.to_string();
                        self.append_to_lines(&error_message)?;
                    }
                }
            }
        }

        self.rebuild_line_views();

        Ok(())
    }

    fn process_control_character(&mut self, control_character: char) {
        use ascii::{AsciiChar, ToAsciiChar};

        match control_character.to_ascii_char().unwrap() {
            AsciiChar::LineFeed => self.execute_command().unwrap(),
            AsciiChar::ETX => {}                                                            // NOTE: control-c
            AsciiChar::EOT => self.delete_character_at_cursor(),                            // NOTE: control-d
            AsciiChar::BackSpace | AsciiChar::DEL => self.delete_character_before_cursor(), // NOTE: for some reason pressing backspace produces DEL. actual delete key is processed in `process_key`
            _ => {}
        }
    }

    fn advance_cursor_down(&mut self) {
        self.cursor_position.y = (self.cursor_position.y + 1).clamp(0, self.maximum_possible_y());

        if !self.is_y_on_screen(self.cursor_position.y) {
            self.scroll_offset += 1;
        }
    }

    fn advance_cursor_left(&mut self) {
        self.cursor_position.x -= 1;

        if self.cursor_position.x < 0 && self.cursor_position.y > 0 {
            self.cursor_position.x = self.window_size.columns as i32;
            self.advance_cursor_up();
        }
    }

    fn advance_cursor_up(&mut self) {
        self.cursor_position.y = (self.cursor_position.y - 1).clamp(0, self.maximum_possible_y());

        if !self.is_y_on_screen(self.cursor_position.y) {
            self.scroll_offset -= 1;
        }
    }

    fn advance_cursor_right(&mut self) {
        self.cursor_position.x += 1;

        if self.cursor_position.x >= self.window_size.columns as i32 {
            self.advance_cursor_down();
            self.cursor_position.x = 0;
        }
    }

    fn focused_command_view_index(&self) -> Option<usize> {
        self.command_views.iter().position(|view| view.y == self.cursor_position.y)
    }

    fn command_view_width(&self, index: usize) -> i32 {
        if index == 0 {
            self.command_views[index].width + self.prompt.len() as i32
        } else {
            self.command_views[index].width
        }
    }

    fn command_index_at_cursor(&self) -> Option<usize> {
        let view_index = self.focused_command_view_index();
        if view_index.is_none() {
            let maximum_command_view_y = self.command_views.iter().map(|view| view.y).max().unwrap_or(0);
            if self.command_views.len() == 0 {
                return Some(0);
            } else if self.cursor_position.y > maximum_command_view_y && self.command_view_width(self.command_views.len() - 1) == self.window_size.columns {
                return Some(self.command.len() - 1);
            } else {
                return None;
            }
        }
        let view_index = view_index.unwrap();

        if view_index == 0 && self.cursor_position.x < self.prompt.len() as i32 {
            return None;
        }

        let index: isize = self.command_views[view_index].offset as isize
            + if view_index == 0 {
                self.cursor_position.x as isize - self.prompt.len() as isize
            } else {
                self.cursor_position.x as isize
            };

        let index = index as usize;
        Some(index.clamp(0, self.command.len())) // just in case
    }

    fn insert_character_in_command_at_cursor(&mut self, character: WideChar) {
        if let Some(index) = self.command_index_at_cursor() {
            self.command.insert(index, character);
        }
    }

    fn process_character(&mut self, character: WideChar) -> Result<(), ncursesw::NCurseswError> {
        if let Ok(control_character) = character.as_char() {
            if control_character.is_control() {
                self.process_control_character(control_character);
                return Ok(());
            }
        }

        if self.focused_line_view().is_none() {
            self.insert_character_in_command_at_cursor(character);
            self.rebuild_command_views();
            self.advance_cursor_right();
        }

        Ok(())
    }

    fn is_cursor_on_command_prompt(&self) -> bool {
        if self.running_child.is_some() {
            false
        } else if self.line_views.len() == 0 {
            true
        } else {
            self.cursor_position.y > (self.line_views.last().unwrap().y)
        }
    }

    fn focused_line_view(&self) -> Option<&LineView> {
        self.line_views.iter().find(|&view| view.y == self.cursor_position.y)
    }

    fn clamp_cursor(&mut self) {
        let maximum_possible_x = (self.window_size.columns - 1) as usize;
        let maximum_allowed_x = (if self.is_cursor_on_command_prompt() {
            if let Some(index) = self.focused_command_view_index() {
                if index == 0 {
                    self.prompt.len() + self.command_views[index].width as usize
                } else {
                    self.command_views[index].width as usize
                }
            } else {
                self.prompt.len()
            }
        } else if let Some(line_view) = self.focused_line_view() {
            (line_view.width) as usize
        } else {
            maximum_possible_x
        })
        .clamp(0, maximum_possible_x);

        self.cursor_position.x = self.cursor_position.x.clamp(0, maximum_allowed_x as i32);
        self.cursor_position.y = self.cursor_position.y.clamp(0, self.window_size.lines as i32 + self.scroll_offset);
    }

    fn append_to_lines(&mut self, string: &str) -> Result<(), ncursesw::NCurseswError> {
        let mut new_line: Vec<ComplexChar> = Vec::new();
        for character in string.chars() {
            if character == '\n' {
                self.lines.push(new_line);
                new_line = Vec::new();
            } else {
                new_line.push(ComplexChar::from_char(character, &self.attributes, &self.color_pair)?);
            }
        }

        if new_line.len() > 0 {
            self.lines.push(new_line);
        }

        Ok(())
    }

    fn read_from_child(&mut self) -> Result<(), ncursesw::NCurseswError> {
        use std::io::Read;

        let mut output_buffer = String::new();
        let child = self.running_child.as_mut().unwrap();
        let stdout = child.stdout.as_mut().unwrap();

        match stdout.read_to_string(&mut output_buffer) {
            Ok(_) => {}
            Err(error) => panic!("cannot read child's stdout into string: {}", error),
        }
        let output_buffer = output_buffer;

        if output_buffer.len() == 0 {
            return Ok(());
        }

        self.append_to_lines(&output_buffer)?;
        self.rebuild_line_views();

        Ok(())
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

        if self.running_child.is_some() {
            self.read_from_child()?;
        }

        if let Some(child) = self.running_child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    drop(child);
                    self.running_child = None;
                }
                Ok(None) => {}
                Err(error) => panic!("cannot wait for the child: {}", error),
            }
        }

        self.clamp_cursor();
        return Ok(self.should_exit);
    }

    fn maximum_possible_y(&self) -> i32 {
        let maximum_line_views_y = if self.line_views.len() > 0 { self.line_views.last().unwrap().y } else { 0 };
        let maximum_command_views_y = if self.command_views.len() > 0 { self.command_views.last().unwrap().y } else { 0 };

        if self.running_child.is_some() {
            maximum_line_views_y
        } else if self.command_views.len() == 0 && self.line_views.len() > 0 {
            maximum_line_views_y + 1
        } else {
            maximum_command_views_y + 1
        }
    }

    fn is_y_on_screen(&self, y: i32) -> bool {
        (y >= self.scroll_offset) && ((y - self.scroll_offset) < self.window_size.lines)
    }

    fn render_lines(&self) -> Result<(), ncursesw::NCurseswError> {
        for view in &self.line_views {
            if !self.is_y_on_screen(view.y) {
                continue;
            }

            for i in 0..view.width as usize {
                mvwins_wch(
                    self.window,
                    Origin {
                        x: i as i32,
                        y: view.y - self.scroll_offset,
                    },
                    self.lines[view.index][view.offset + i],
                )?;
            }
        }

        Ok(())
    }

    fn render_command(&self) -> Result<(), ncursesw::NCurseswError> {
        if self.running_child.is_some() {
            return Ok(());
        }

        let prompt_y = if self.line_views.len() > 0 { self.line_views.last().unwrap().y + 1 } else { 0 };
        if !self.is_y_on_screen(prompt_y) {
            return Ok(());
        }

        wmove(
            self.window,
            Origin {
                x: 0,
                y: prompt_y - self.scroll_offset,
            },
        )?;
        waddstr(self.window, self.prompt)?;

        let mut first_line: bool = true;
        for view in &self.command_views {
            let y = view.y - self.scroll_offset;

            if !self.is_y_on_screen(view.y) {
                continue;
            }
            let x_offset = if first_line { self.prompt.len() as i32 } else { 0 };
            first_line = false;

            for i in 0..view.width as usize {
                mvwins_wch(
                    self.window,
                    Origin { x: x_offset + i as i32, y },
                    ComplexChar::from_wide_char(self.command[view.offset + i], &self.attributes, &self.color_pair)?,
                )?;
            }
        }

        Ok(())
    }

    pub fn render(&self) -> Result<(), ncursesw::NCurseswError> {
        wclear(self.window)?;

        self.render_lines()?;
        self.render_command()?;

        let screen_cursor_position = Origin {
            x: self.cursor_position.x,
            y: self.cursor_position.y - self.scroll_offset,
        };

        wmove(self.window, screen_cursor_position)?;

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

fn parse_command(command: &Vec<WideChar>) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    let mut current_token = String::new();

    for wide_character in command.iter() {
        let character = wide_character.as_char().expect("BUG: something not convertable to char got into `command` vector");
        if character.is_whitespace() {
            result.push(current_token);
            current_token = String::new();
        } else if character == '#' {
            break;
        } else {
            current_token.push(character);
        }
    }

    if current_token.len() > 0 {
        result.push(current_token);
    }

    result
}
