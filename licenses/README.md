# Maintaining license notices

Distribute `LICENSE` and `THIRD_PARTY_NOTICES.md` with release binaries. The binary
also embeds both files: `annodiff --licenses` works after `cargo install`, without
a source checkout or network access.

After changing dependencies, fetch the locked sources and regenerate notices:

```sh
cargo fetch --locked
python3 licenses/generate.py
python3 licenses/generate.py --check
```

The generator uses Python's standard library and Cargo metadata. It includes all
resolved crates, including optional backends and other targets, deduplicates
identical texts, and retains upstream attribution files. It selects permissive
alternatives where offered (`MIT` for `r-efi` and `termina`). Unknown license
expressions, missing texts, changed supplemental versions, and changed syntect
asset hashes stop generation for review. This checks the recorded inventory;
new dependencies and embedded assets still need a source-level license review.

## Supplement provenance

`supplemental.json` preserves upstream texts missing from crate archives, with
source URLs pinned to commits where available. The WezTerm and Ratatui commits
come from the published crates' `.cargo_vcs_info.json`. The two winapi import
library crates carry their own copyright headers and refer to the winapi-rs MIT
license; both the headers and that license are included. `r-efi` carries its MIT
terms and copyright holders in `AUTHORS`, collected directly from the crate.

For syntect 5.3.0, the published source commit is
`e4670846ecf16d8832db6c43d531bec466214e27`. Its Makefile generates syntax packs from
`testdata/Packages` and the theme pack from themes under `testdata`. We checked
the submodule revisions both at that release and at the commits that last
generated `default_newlines.packdump` (`47895669578ca17511aafb854057abcfad1edfc1`)
and `default.themedump` (`dd4947cf69d52ae1b44d5162bdcf9122c1fb1576`); they agree:

| Source | Commit | Terms |
|---|---|---|
| sublimehq/Packages | `fa6b8629c95041bf262d4c1dab95c456a0530122` | Permissive repository license; MIT notices for C#, Rust and YAML |
| sethlopezme/InspiredGitHub.tmtheme | `18ddb271179e118cfc2dd83abf88b915b7328a25` | MIT |
| braver/Solarized | `bcd6234b4f5f96d3fd27db079268b5757053072a` | MIT |
| kkga/spacegray | `2703e93f559e212ef3895edd10d861a4383ce93d` | MIT; theme author credits also retained |

The supplemental records include their license texts and individual syntax
headers. C# names MIT by URL; the full MIT permission and disclaimer are included
in the other MIT notices. Hashes pin the published syntect assets being reviewed.
No mandatory copyleft terms were found in this inventory. Recheck the upstream
sources and update these records when replacing syntect or its bundled data.
