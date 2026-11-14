# Velocity palettes

Original palettes and rule sets, MIT OR Apache-2.0. Designed from a blue-grey ink/paper pair:
Dark uses a quiet navy ground and bright mineral accents; Light uses warm paper and darker
versions of the same semantic colors. These are our own numeric palettes, not adaptations of
another scheme. Comments stay readable and use italics; punctuation retains normal foreground
contrast. Error underlines remain distinguishable without relying on hue alone.

| Role | Dark | Contrast | Light | Contrast |
| --- | --- | ---: | --- | ---: |
| Background | #14232e | — | #f7f5ed | — |
| Foreground | #e3ebee | 13.27 | #263743 | 11.25 |
| Comments | #8b9da5 | 5.70 | #607279 | 4.60 |
| Functions, names, headings, links | #83c5e8 | 8.49 | #175c8a | 6.56 |
| Types, namespaces, support | #7dd2c2 | 9.06 | #176557 | 6.34 |
| Strings, raw markup, insertions | #a6ce8d | 9.04 | #416824 | 5.96 |
| Numbers, constants, changes | #efc17b | 9.61 | #815409 | 6.00 |
| Keywords, storage, tags | #c9acf1 | 8.14 | #69419b | 6.80 |
| Parameters, language constants | #e9a8c0 | 8.27 | #8a375d | 6.91 |
| Invalid code, deletions | #ff9b93 | 7.91 | #a3312e | 6.34 |

Ratios use WCAG sRGB relative luminance against the scheme background. Every normal token
exceeds 4.5:1, and comments exceed 3:1; tests verify these thresholds through the actual style
resolver. Selection uses a separate blue-grey surface and explicit foreground. Low-contrast
surfaces and guides are decoration rather than normal text.

The rules use the public scope naming families (comment, constant, entity, invalid, keyword,
markup, punctuation, storage, string, support, variable). Source/text/meta containers inherit
the document foreground; specific names, tag punctuation, interpolation escapes, markup
emphasis, links, diffs, and invalid/deprecated tokens refine those families. Both schemes can
be used with all 202 loaded official syntax definitions without language-specific palette
copies. This does not claim visual inspection of every token in every language.
