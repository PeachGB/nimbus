fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let mut terminal = ratatui::init();
    let result = nimbus_creator::run(&mut terminal);
    ratatui::restore();
    match result? {
        Some(path) => println!("saved vault config to {}", path.display()),
        None => println!("cancelled"),
    }
    Ok(())
}
