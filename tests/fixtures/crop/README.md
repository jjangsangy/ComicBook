# Cropping fixtures

Small synthetic pages used by `tests/ebook_crop_tests.rs` to pin the Rust
cropping port to KCC's own behaviour.

Each fixture was fed once through KCC's own cropping entry points —
`get_bbox_crop_margin`, `get_bbox_crop_margin_page_number` and
`crop_empty_inter_panel` — run in the throwaway Python environment described in
AGENTS.md §20.4. The values asserted in the Rust test are exactly what those
functions returned (KCC 11.3.2; the boxes are stable across Pillow/NumPy
versions — they were reproduced identically under both Pillow 11.3.0/NumPy 2.0.2
and Pillow 12.3.0/NumPy 2.4.6).

The fixtures are pure black/white so that the grayscale conversion, autocontrast,
blur and threshold are bit-exact across Pillow and the Rust implementation; the
crop geometry is therefore reproduced exactly rather than approximately. They were
built as plain arrays (white pages with black rectangles), not rendered artwork.

| Fixture | Size | Content |
|:---|:---|:---|
| `margin-white.png` | 200x300 | White page, black frame `x∈[20,180)`, `y∈[30,270)` |
| `pagenum-white.png` | 800x1200 | White page, black artwork `x∈[80,720)`, `y∈[100,1100)`, page number `x∈[380,404)`, `y∈[1150,1170)` |
| `pagenum-black.png` | 800x1200 | `pagenum-white.png` inverted (black background) |
| `interpanel-white.png` | 200x300 | White page, black panels `y∈[20,140)` and `y∈[160,280)` within `x∈[20,180)` |

## Reference values

`margin-white.png`, white background:

| Power | margin box | after the 10 % clamp |
|:---|:---|:---|
| 0.0 | `(19, 29, 181, 271)` | `(19, 29, 181, 271)` |
| 1.0 | `(19, 29, 181, 271)` | `(19, 29, 181, 271)` |
| 2.0 | `(20, 30, 180, 270)` | `(20, 30, 180, 270)` |
| 3.0 | `(21, 31, 179, 269)` | `(20, 30, 180, 270)` |

`pagenum-white.png` (power 1.0): margin `(79, 99, 721, 1171)`, page number
`(79, 99, 721, 1101)`. At power 2.0 they are `(80, 100, 720, 1170)` and
`(80, 100, 720, 1100)`.

`pagenum-black.png` (black background, power 1.0): margin `(79, 99, 721, 1171)`,
page number `(79, 99, 721, 1101)`.

`interpanel-white.png`: `horizontal` → 285 rows, `vertical` → 184 columns,
`both` → 285x184. The gutter cutter drops rows 142–156 and columns 182–197.
