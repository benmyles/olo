//! A one-time code's set-up QR code read from a picture of a page: the
//! `otpauth://` URI a site shows to be scanned with a phone, found in a
//! picture of the tab (Chrome's extension sends one when a page looks to
//! be showing such a code) and read here, natively (rqrr): nothing of the
//! picture is kept or leaves the computer.

/// The most a picture may be (a 5K display's tab is 15 million pixels).
const MOST_PIXELS: u64 = 40_000_000;

/// The `otpauth://` URI in a QR code the picture `file` (a PNG or JPEG
/// file's bytes) shows, if it shows one: the first of them. Every QR
/// code in the picture is read; one that says something else (a link, a
/// Wi-Fi network) isn't it.
pub fn otpauth_in(file: &[u8]) -> Option<String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(file)).with_guessed_format().ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    if u64::from(w) * u64::from(h) > MOST_PIXELS {
        return None;
    }
    let luma = image::load_from_memory(file).ok()?.to_luma8();
    let mut picture = rqrr::PreparedImage::prepare(luma);
    picture.detect_grids().iter().filter_map(|grid| grid.decode().ok()).map(|(_, text)| text).find(|text| is_otpauth(text))
}

/// Whether `text` is a one-time code's set-up URI that Passwords keeps: a
/// time-based one (`otpauth://totp/…`) with a secret.
pub fn is_otpauth(text: &str) -> bool {
    text.trim().get(..15).is_some_and(|s| s.eq_ignore_ascii_case("otpauth://totp/")) && onepassword::totp::Totp::parse(text).is_ok_and(|t| !t.secret.is_empty())
}

/// Which login a code read from a page goes with, of those kept for the
/// site (each: its username, whether it has a one-time code already):
/// the one the code names (its account), else the only one; None when
/// that doesn't say (several, none named), for the person to choose.
pub fn joins(account: Option<&str>, logins: &[(String, bool)]) -> Option<usize> {
    let named: Vec<usize> = match account.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => logins.iter().enumerate().filter(|(_, (user, _))| user.eq_ignore_ascii_case(a)).map(|(i, _)| i).collect(),
        None => Vec::new(),
    };
    match (named.as_slice(), logins.len()) {
        ([one], _) => Some(*one),
        (_, 1) => Some(0),
        _ => None,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A picture of a page with `text` as a QR code on it (a PNG file):
    /// white, the code `scale` pixels a module at (`x`, `y`).
    pub fn page_with(text: &str, scale: u32, (x, y): (u32, u32), (w, h): (u32, u32)) -> Vec<u8> {
        let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
        let n = code.width() as u32;
        let dark = code.to_colors();
        let mut page = image::GrayImage::from_pixel(w, h, image::Luma([255]));
        for (i, c) in dark.iter().enumerate() {
            if *c == qrcode::Color::Dark {
                let (cx, cy) = (i as u32 % n, i as u32 / n);
                for dy in 0..scale {
                    for dx in 0..scale {
                        page.put_pixel(x + cx * scale + dx, y + cy * scale + dy, image::Luma([20]));
                    }
                }
            }
        }
        let mut out = Vec::new();
        image::DynamicImage::ImageLuma8(page).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    pub const URI: &str = "otpauth://totp/Example:ann@example.com?secret=JBSWY3DPEHPK3PXP&issuer=Example";

    #[test]
    fn a_set_up_code_is_read_from_a_page() {
        // Somewhere on a page as big as a tab, at a size a site shows it.
        let page = page_with(URI, 4, (700, 320), (1600, 1000));
        assert_eq!(otpauth_in(&page).as_deref(), Some(URI));
        // Small, and as a JPEG (the extension's picture).
        let small = page_with(URI, 3, (40, 40), (400, 300));
        let mut jpeg = Vec::new();
        image::load_from_memory(&small).unwrap().write_to(&mut std::io::Cursor::new(&mut jpeg), image::ImageFormat::Jpeg).unwrap();
        assert_eq!(otpauth_in(&jpeg).as_deref(), Some(URI));
        // A QR code that says something else, no code at all, not a picture.
        assert_eq!(otpauth_in(&page_with("https://example.com/download", 4, (10, 10), (300, 300))), None);
        assert_eq!(otpauth_in(&page_with("otpauth://hotp/Example:ann?secret=JBSWY3DPEHPK3PXP&counter=1", 4, (10, 10), (400, 400))), None);
        let mut blank = Vec::new();
        image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(200, 200, image::Luma([255]))).write_to(&mut std::io::Cursor::new(&mut blank), image::ImageFormat::Png).unwrap();
        assert_eq!(otpauth_in(&blank), None);
        assert_eq!(otpauth_in(b"not a picture"), None);
        assert!(is_otpauth(URI) && is_otpauth("OTPAUTH://TOTP/x?secret=JBSWY3DPEHPK3PXP"));
        assert!(!is_otpauth("otpauth://totp/x") && !is_otpauth("JBSWY3DPEHPK3PXP") && !is_otpauth(""));
    }

    /// A QR code's picture for trying the extension with a page of one's
    /// own: `URI` (or `OLO_FIXTURE_QR_URI`) as a PNG at `OLO_FIXTURE_QR`.
    /// `cargo test --release -p olo-passwords qr_fixture -- --ignored`.
    #[test]
    #[ignore]
    fn qr_fixture() {
        let Some(file) = std::env::var_os("OLO_FIXTURE_QR") else { return };
        let uri = std::env::var("OLO_FIXTURE_QR_URI").unwrap_or_else(|_| URI.to_owned());
        std::fs::write(file, page_with(&uri, 6, (24, 24), (360, 360))).unwrap();
    }

    #[test]
    fn which_login_a_code_goes_with() {
        let l = |user: &str, has: bool| (user.to_owned(), has);
        // The one the code names, whatever the others.
        assert_eq!(joins(Some("ann@example.com"), &[l("bob", false), l("Ann@Example.com", true)]), Some(1));
        // The only one for the site, named or not.
        assert_eq!(joins(None, &[l("ann", false)]), Some(0));
        assert_eq!(joins(Some("someone-else"), &[l("ann", false)]), Some(0));
        // Several and none named (or two of the name): the person's to say.
        assert_eq!(joins(Some("carol"), &[l("ann", false), l("bob", false)]), None);
        assert_eq!(joins(Some("ann"), &[l("ann", false), l("ann", true)]), None);
        assert_eq!(joins(None, &[]), None);
    }
}
