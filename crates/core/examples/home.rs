//! Checks the home feed without an account: `cargo run -p ytm-core --example home`

use ytm_core::Client;

fn main() -> anyhow::Result<()> {
    let client = Client::new(std::env::temp_dir().join("ytm-core-examples"))?;
    let feed = tokio::runtime::Builder::new_current_thread()
        .build()?
        .block_on(client.home())?;
    println!("trending: {}", feed.trending.len());
    for track in feed.trending.iter().take(3) {
        println!("  {} — {}", track.title, track.artists);
    }
    println!("top: {}", feed.top.len());
    println!("artists: {}", feed.artists.len());
    println!("new albums: {}", feed.new_albums.len());
    Ok(())
}
