fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let mut terminal = ratatui::init();
    let result = nimbus_creator::run_for_outcome(&mut terminal);
    ratatui::restore();
    // After the restore: printed inside the alternate screen, it would vanish with it.
    let outcome = result?;
    if let Some(key_file) = &outcome.unsaved_key_file {
        eprintln!("{}", nimbus_creator::kept_key_notice(key_file));
    }
    match outcome.config {
        Some(path) => println!("saved vault config to {}", path.display()),
        None => println!("cancelled"),
    }
    Ok(())
}
