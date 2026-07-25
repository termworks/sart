use bootart::DEFAULT_LOGO;
use bootart::art::Art;

fn main() {
    let art = Art::parse(DEFAULT_LOGO).expect("valid embedded art");
    println!("Embedded logo size: {}x{}", art.width, art.height);
}
