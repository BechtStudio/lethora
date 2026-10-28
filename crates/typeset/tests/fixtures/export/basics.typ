#outline()

= Introduction <intro>

Text with *bold*, _italic_, #underline[underlined], #strike[struck], H#sub[2]O, x#super[2], #smallcaps[Small Caps], `code` and #link("https://example.com")[a link] and https://typst.app.
A forced\
line break, "quotes" and 5 < 6 & 7 > 3.

== Lists

- one
- two
  - nested

+ first
+ second

#enum(start: 4)[four][five]

/ Term: Its description.

#quote(block: true)[A quotation.]

```rust
fn main() {
    println!("hi");
}
```

#line(length: 100%)

#pagebreak()

#styled("missing")[Styled text] stays text.
