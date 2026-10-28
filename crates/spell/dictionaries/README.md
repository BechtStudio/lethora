# Bundled dictionaries

Hunspell dictionaries (UTF-8), taken unchanged from
[wooorm/dictionaries](https://github.com/wooorm/dictionaries) at commit
`8cfea406b505e4d7df52d5a19bce525df98c54ab` (2026-10-26), except de-CH:
wooorm's de-CH still spells with ß, so it comes from
[LibreOffice/dictionaries](https://github.com/LibreOffice/dictionaries)
(`de/de_CH_frami.*` at commit `67032ee957631908098a72edf48dc81cac27e369`,
2026-10-26), converted from ISO-8859-1 to UTF-8 (`SET UTF-8`, nothing else
changed; its `license` is LibreOffice's `README_de_DE_frami.txt`). Each
folder keeps its upstream `license` file; the files are not covered by
Lethora's own license.

| Folder | Upstream | Source | License |
|---|---|---|---|
| `en-US` | `dictionaries/en` | SCOWL en_US 2020.12.07 (wordlist.aspell.net) | MIT AND BSD |
| `en-GB` | `dictionaries/en-GB` | SCOWL en_GB-ise 2020.12.07 | MIT AND BSD |
| `de-DE` | `dictionaries/de` | igerman98 (j3e.de), Björn Jacke | GPL-2.0 OR GPL-3.0 |
| `de-AT` | `dictionaries/de-AT` | igerman98 | GPL-2.0 OR GPL-3.0 |
| `de-CH` | LibreOffice `de/de_CH_frami` | igerman98 + frami additions (Franz Michael Baumann) | GPL-2.0 OR GPL-3.0 |

SHA-256 of the files as taken (de-CH: the ISO-8859-1 originals
`d86884be…4126b` aff, `80a4df38…bd16` dic):

```
8ae1f19d4840d957728ad90555d5a8dff6cc5c046279c95ff0c00fc0a0136c7b  en-US/index.aff
f0b1a234bd178bdd01875b2a392a9647f888b8fe879f79c52aae62c2759b3647  en-US/index.dic
8ae1f19d4840d957728ad90555d5a8dff6cc5c046279c95ff0c00fc0a0136c7b  en-GB/index.aff
869fe17ba4ee4b5401c60a666ee2d6a3dcc237f460b7294435df1cc6a799aa57  en-GB/index.dic
57fdd1b16aac2131003c91e0cf2a488becb970382a402a9ce089307301cb3ef0  de-DE/index.aff
b5c781a0cf6f285fb6b9b8ab02fbea104b987104a1efdda8a835837e89e3ec77  de-DE/index.dic
04ebc966a0b1130d8fc62886bb801e77bf7d6a11accf205954c364d3c0c68864  de-AT/index.aff
02cbbeb74af85497b861b5782413b6c8c8ffab5c9423ef2e92265599b9359870  de-AT/index.dic
dc312eb01c299b489aa384af2cc1f521af06be38c950ba7c5e75e549c6fbbc09  de-CH/index.aff
31c7e7696916bf7f0a163a3185df38887d6087883421241777f5704aaf542147  de-CH/index.dic
```
