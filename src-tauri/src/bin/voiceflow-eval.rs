//! Research-only file evaluation; does not initialize the desktop application.
#[tokio::main]
async fn main() {
    let code = match tauri_app_lib::evaluation::main(std::env::args().skip(1).collect()).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::to_string(&error).unwrap_or_else(|_| {
                    "evaluation error serialization unavailable; attempt facts unknown".into()
                })
            );
            if std::env::args()
                .any(|arg| matches!(arg.as_str(), "--filter-evidence" | "--filter-report"))
            {
                1
            } else {
                error.exit_code()
            }
        }
    };
    std::process::exit(code);
}
