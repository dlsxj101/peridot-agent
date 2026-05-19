use super::*;

pub(super) fn print_json_or_text_result(
    value: serde_json::Value,
    text: String,
    output: OutputFormat,
) -> Result<()> {
    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&value)?),
        OutputFormat::Text => println!("{text}"),
    }
    Ok(())
}
