---
title: "Export test"
subtitle: "Every construct"
date: "26 October 2026"
lang: "en"
author: "Ada Lovelace"
---

# 1. Introduction {#intro}

Text with **bold**, *italic*, underlined, ~~struck~~, H<sub>2</sub>O, x<sup>2</sup>, Small Caps, `code` and [a link](https://example.com) and <https://typst.app>. A forced\
line break, “quotes” and 5 \< 6 & 7 > 3.

## 1.1. Lists

- one
- two
  - nested

<!-- -->

1. first
2. second

<!-- -->

4. four
5. five

**Term**

Its description.

> A quotation.

```rust
fn main() {
    println!("hi");
}
```

Styled text stays text.

# 2. Method {#method}

See Section 2, Figure 1, Table 1 and Equation 1.

$$e^(i pi) + 1 = 0$$

![Figure 1: A plot.](export_images/plot.svg)

Table 1: A table.

|  |  |
| --- | --- |
| a | b |

As (Knuth, 1984) shows, and (Doe & Roe, 2020, p. 4) agrees. Both at once: (Doe & Roe, 2020; Knuth, 1984).

Text.[^1] More text.[^2]

![Figure 2: A red square.](export_images/red.png)

Table 2: Values.

| Name | Value |
| --- | --- |
| a | 1 |

![](export_images/plot.svg)

An inline ![](export_images/red.png) icon.

```py
print(1)
```

Listing 1: Code.

| A | B | C |
| --- | --- | --- |
| wide |  | x |
| tall | 1 | 2 |
|  | 3 | **4** |

|  |  |
| --- | --- |
| one | two<br>paragraphs |

Inline $a^2 + b^2 = c^2$ and $x < y$.

$$sum_(k=1)^n k = (n(n+1)) / 2$$

$$a &= b \ c &= d$$

# Bibliography

- Doe, J., & Roe, R. (2020). On Layout. *Journal of Typesetting*.
- Knuth, D. E. (1984). *The TeXbook*. Addison-Wesley.

[^1]: A note with **bold** and (Knuth, 1984).

[^2]: Second note.

    With a second paragraph.
