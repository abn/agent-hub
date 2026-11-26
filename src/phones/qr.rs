//! The pairing URI as a QR code the web app draws.

use serde::Serialize;

use crate::error::{Error, Result};

/// The dark modules of a QR code, as rows of runs, so the web app draws it
/// with its own colours and no image crosses the wire.
#[derive(Debug, Clone, Serialize)]
pub struct Matrix {
    /// Modules per side, without the quiet zone.
    pub size: usize,
    /// One SVG path over a `size` by `size` grid: a unit-high rectangle per
    /// run of dark modules.
    pub path: String,
}

impl Matrix {
    /// Encode `text` at error correction M, the level Tendril sizes its
    /// pairing URIs for.
    pub fn encode(text: &str) -> Result<Self> {
        let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M).map_err(
            |err| Error::InvalidArgument(format!("the pairing URI does not fit a QR code: {err}")),
        )?;
        let size = code.width();
        let colors = code.to_colors();
        let mut path = String::new();
        for (y, row) in colors.chunks(size).enumerate() {
            let mut x = 0;
            while x < size {
                if row[x] == qrcode::Color::Dark {
                    let start = x;
                    while x < size && row[x] == qrcode::Color::Dark {
                        x += 1;
                    }
                    path.push_str(&format!("M{start} {y}h{}v1H{start}z", x - start));
                } else {
                    x += 1;
                }
            }
        }
        Ok(Self { size, path })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dark_module_is_drawn_once_and_the_grid_is_square() {
        let code =
            qrcode::QrCode::with_error_correction_level("tendril://pair?v=1", qrcode::EcLevel::M)
                .expect("encodes");
        let dark = code
            .to_colors()
            .iter()
            .filter(|c| **c == qrcode::Color::Dark)
            .count();
        let matrix = Matrix::encode("tendril://pair?v=1").expect("encodes");
        assert_eq!(matrix.size, code.width());
        let drawn: usize = matrix
            .path
            .split('M')
            .filter(|run| !run.is_empty())
            .map(|run| {
                let width = run
                    .split('h')
                    .nth(1)
                    .and_then(|rest| rest.split('v').next());
                width
                    .and_then(|w| w.parse::<usize>().ok())
                    .expect("a run width")
            })
            .sum();
        assert_eq!(drawn, dark);
    }
}
