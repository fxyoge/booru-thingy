fn main() {
    // Track directory changes too, so adding/removing assets reruns the embedding macro.
    println!("cargo:rerun-if-changed=assets");
}
