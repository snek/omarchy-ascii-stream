use bootleg_play::decode::Error;
use bootleg_play::footer::Meta;
use bootleg_play::render::Frame;

fn main() {
    let exe = std::env::current_exe().expect("exe");
    let bytes = std::fs::read(&exe).expect("read exe");
    if let Some((meta, range)) = bootleg_play::footer::read(&bytes) {
        let frames = frames_from_payload(&meta, &bytes[range]);
        std::process::exit(bootleg_play::play::play(&meta, &frames));
    }
    match bootleg_play::decode::run_args(std::env::args().skip(1)) {
        Ok(()) => {}
        Err(Error::Usage) => {
            eprintln!("bootleg-play export --video CLIP -o PATH [--fps N] [--grid-cols N]");
            std::process::exit(2);
        }
        Err(Error::Fail(msg)) => {
            eprintln!("bootleg-play: {msg}");
            std::process::exit(1);
        }
    }
}

fn frames_from_payload(meta: &Meta, payload: &[u8]) -> Vec<Frame> {
    let width = meta.ref_cols as usize;
    let height = meta.ref_rows as usize;
    let size = width.saturating_mul(height).saturating_mul(3);
    if size == 0 {
        return Vec::new();
    }
    payload
        .chunks_exact(size)
        .map(|rgb| Frame {
            width,
            height,
            rgb: rgb.to_vec(),
        })
        .collect()
}
