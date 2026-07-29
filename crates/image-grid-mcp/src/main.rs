use std::io;

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    if let Err(error) = image_grid_mcp::serve(stdin.lock(), stdout.lock()) {
        eprintln!("image-grid-mcp: {error}");
        std::process::exit(1);
    }
}
