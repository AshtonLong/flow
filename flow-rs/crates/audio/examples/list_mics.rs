//! Prints the microphones as the settings window lists them.
fn main() {
    for mic in flow_audio::list_microphones() {
        println!("{}\t{}", mic.label, mic.id);
    }
}
