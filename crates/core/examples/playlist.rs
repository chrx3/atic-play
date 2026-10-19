//! Opens a playlist with the saved session: `cargo run -p ytm-core --example playlist -- <id>`
//! Copies the app's `cookies.txt` into a temp dir, so the app's own cache is left alone.

use ytm_core::Client;

fn main() -> anyhow::Result<()> {
    let id = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("falta el id de la playlist"))?;
    let dir = std::env::temp_dir().join("ytm-core-examples-session");
    std::fs::create_dir_all(&dir)?;
    let app = std::path::PathBuf::from(std::env::var("APPDATA")?).join("atic-play");
    std::fs::copy(app.join("cookies.txt"), dir.join("cookies.txt"))?;
    let client = Client::new(&dir)?;
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    println!("sesión: {}", runtime.block_on(client.restore_session())?);
    let page = runtime.block_on(client.playlist(id))?;
    println!("{} · {} canciones", page.playlist.title, page.tracks.len());
    for track in page.tracks.iter().take(5) {
        println!("  {} — {}", track.title, track.artists);
    }
    Ok(())
}
