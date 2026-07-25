use bootart::art::Art;
use bootart::renderer::{play_animation, RenderOptions};
use bootart::terminal::StdoutTerminal;
use bootart::{DEFAULT_LOGO, SMALL_LOGO};
use std::fs;
use std::path::Path;
use std::process::exit;

fn main() {
    let logo_path = Path::new("/etc/bootart/logo.txt");
    let logo_str = if logo_path.exists() {
        fs::read_to_string(logo_path).unwrap_or_else(|_| DEFAULT_LOGO.to_string())
    } else {
        DEFAULT_LOGO.to_string()
    };

    let art = match Art::parse(&logo_str) {
        Ok(a) => a,
        Err(_) => match Art::parse(DEFAULT_LOGO) {
            Ok(a) => a,
            Err(_) => exit(1),
        },
    };

    let small_art = Art::parse(SMALL_LOGO).ok();
    let mut term = StdoutTerminal::with_override(None, None);
    let options = RenderOptions {
        duration_ms: 2500,
        fps: 30,
        seed: 42,
        no_color: false,
        clear_first: true,
        leave_final: true,
    };

    if let Err(e) = play_animation(&mut term, &art, small_art.as_ref(), options, 0) {
        eprintln!("Render error: {}", e);
        exit(1);
    }

    if std::process::id() == 1 {
        unsafe {
            libc::reboot(libc::RB_POWER_OFF);
            libc::reboot(libc::RB_HALT_SYSTEM);
        }
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
}
