# Hyphenation dictionaries

Liang patterns from the TeX hyph-utf8 project, compiled to bincode by the
[`hyphenation`](https://crates.io/crates/hyphenation) crate (0.8.4). Each file
keeps the license of its original pattern set (see
https://github.com/hyphenation/tex-hyphen); several, including Italian, are
LPPL. They are shipped as separate data files loaded at runtime
(`/mnt/us/omero/hyph/` on the Kindle), never linked into the GPL binary.

File name: `<language>.standard.bincode`. Add a language by copying its file
from the crate's `dictionaries/` directory.
