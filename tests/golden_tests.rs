use bootart::animation::AnimationMetadata;
use bootart::art::{Art, Size, layout};
use bootart::renderer::{FrameOptions, generate_frame_bytes};
use bootart::terminal::TerminalSize;
use bootart::{DEFAULT_LOGO, SMALL_LOGO};
use std::fs;
use std::path::Path;

#[test]
fn literal_art_matches_reviewable_non_product_fixtures() {
    assert_eq!(
        DEFAULT_LOGO.as_bytes(),
        include_str!("../assets/logo.txt").as_bytes()
    );
    assert_eq!(
        SMALL_LOGO.as_bytes(),
        include_str!("../assets/logo-small.txt").as_bytes()
    );
}

#[test]
fn test_golden_frame_generation() {
    let art = Art::parse(
        r#"  ____  
 / __ \ 
/ /_/ / 
/_____/ "#,
    )
    .unwrap();

    let term_size = TerminalSize {
        width: 40,
        height: 10,
    };
    let layout_info = layout(
        art.size(),
        Size {
            width: term_size.width,
            height: term_size.height,
        },
    );
    let meta = AnimationMetadata::new(&art, 42);

    let progress_points = [0.0f32, 0.25, 0.50, 0.75, 1.0];
    let golden_dir = Path::new("tests/golden");
    let update_golden = std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1")
        && std::env::var("BOOTART_GOLDEN_WRITE_TOKEN").as_deref() == Ok("make-update-golden-v1");

    if update_golden {
        fs::create_dir_all(golden_dir).expect("failed to create golden fixture directory");
    }

    for &p in &progress_points {
        let frame_bytes = generate_frame_bytes(
            &art,
            &meta,
            &layout_info,
            FrameOptions {
                progress: p,
                no_color: false,
                first_frame: p == 0.0,
                clear_first: true,
                iteration: 0,
            },
        );
        let frame_str = String::from_utf8_lossy(&frame_bytes);
        let filename = format!("frame_{:03}.ans", (p * 100.0) as u32);
        let golden_file = golden_dir.join(&filename);

        if update_golden {
            fs::write(&golden_file, frame_str.as_bytes()).expect("failed to update golden fixture");
        } else if !golden_file.is_file() {
            panic!(
                "missing golden fixture {}; run `make update-golden` to create it",
                golden_file.display()
            );
        }

        let expected = fs::read_to_string(&golden_file).unwrap();
        assert_eq!(
            frame_str, expected,
            "Golden frame mismatch for progress {}",
            p
        );
    }
}
