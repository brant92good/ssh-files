use anyhow::{Result, ensure};
use base64::Engine;
use std::io::Write;

fn request(text: &str) -> Result<String> {
    ensure!(
        text.len() <= 4096 && !text.chars().any(char::is_control),
        "Path cannot be copied as terminal text"
    );
    Ok(format!(
        "\x1b]52;c;{}\x07",
        base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
    ))
}
pub fn copy(text: &str) -> Result<()> {
    // User-initiated OSC52 request. The terminal owns clipboard permission;
    // this neither reads the clipboard nor spawns shell/clipboard commands.
    let mut output = std::io::stdout().lock();
    output.write_all(request(text)?.as_bytes())?;
    output.flush()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copied_path_is_only_encoded_data_and_control_paths_are_rejected() {
        let text = "/srv/space 開發/quit $;.txt";
        let value = request(text).unwrap();
        let encoded = value
            .strip_prefix("\x1b]52;c;")
            .unwrap()
            .strip_suffix('\x07')
            .unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
            text.as_bytes()
        );
        assert!(request("/srv/file\x1b[20~").is_err());
        assert!(request(&"x".repeat(4097)).is_err());
    }
}
