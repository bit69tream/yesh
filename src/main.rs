mod yesh;
use yesh::{panic_hook, Yesh};

use ncursesw::NCurseswError;

fn real_main() -> Result<(), NCurseswError> {
    let mut yesh = Yesh::new()?;

    while !yesh.process_events()? {
        yesh.render()?;
    }

    Ok(())
}

fn main() {
    use std::panic;

    panic::set_hook(Box::new(panic_hook));

    if let Err(error) = real_main() {
        panic!("{}", error.to_string());
    }
}
