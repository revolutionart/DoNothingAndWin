fn main() {
    let engines = engine_workspace::platform::display_engines();
    for e in engines {
        println!("{} | {} | {}", e.version, e.path, e.association);
    }
}
