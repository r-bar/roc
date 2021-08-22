pub const NOTHING_OPENED: &str = "Opening files is not yet supported. Execute `cargo run edit` from the root folder of the repo to try the editor.";
pub const START_TIP: &str =
    "Start by typing '[', '{', '\"' or a number.\nInput chars that would create parse errors will be ignored.";

pub const HELLO_WORLD: &str = r#"
app "test-app"
    packages { base: "platform" }
    imports []
    provides [ main ] to base

main = "Hello, world!"


"#;

