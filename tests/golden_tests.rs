use bootart::animation::AnimationMetadata;
use bootart::art::{layout, Art, Size};
use bootart::renderer::generate_frame_bytes;
use bootart::terminal::TerminalSize;
use std::fs;
use std::path::Path;

#[test]
fn test_golden_frame_generation() {
    let art = Art::parse(
        r#"  ____  
 / __ \ 
/ /_/ / 
/_____/ "#,
    )
    .unwrap();

    let term_size = TerminalSize { width: 40, height: 10 };
    let layout_info = layout(art.size(), Size { width: term_size.width, height: term_size.height });
    let meta = AnimationMetadata::new(&art, 42);

    let progress_points = [0.0f32, 0.25, 0.50, 0.75, 1.0];
    let golden_dir = Path::new("tests/golden");
    fs::create_dir_all(golden_dir).unwrap();

    let update_golden = std::env::var("UPDATE_GOLDEN").is_ok();

    for &p in &progress_points {
        let frame_bytes = generate_frame_bytes(
            &art,
            &meta,
            &layout_info,
            p,
            false,
            p == 0.0,
            true,
            0,
        );
        let frame_str = String::from_utf8_lossy(&frame_bytes);
        let filename = format!("frame_{:03}.ans", (p * 100.0) as u32);
        let golden_file = golden_dir.join(&filename);

        if update_golden || !golden_file.exists() {
            fs::write(&golden_file, frame_str.as_bytes()).unwrap();
        }

        let expected = fs::read_to_string(&golden_file).unwrap();
        assert_eq!(
            frame_str, expected,
            "Golden frame mismatch for progress {}",
            p
        );
    }
}
