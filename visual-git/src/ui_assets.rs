mod generated {
    include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));
}

const MISSING_UI_HTML: &[u8] = br#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Visual Git UI Missing</title>
</head>
<body>
  <h1>Frontend assets are missing</h1>
  <p>Development: run <code>npm run ui:dev</code> and open <code>http://127.0.0.1:5173</code>.</p>
  <p>Production/release: run <code>npm run ui:build</code> before building Rust.</p>
</body>
</html>
"#;

pub fn get(path: &str) -> Option<&'static [u8]> {
    generated::get(path)
}

pub fn has_assets() -> bool {
    generated::ASSET_COUNT > 0
}

pub fn missing_assets_html() -> &'static [u8] {
    MISSING_UI_HTML
}
