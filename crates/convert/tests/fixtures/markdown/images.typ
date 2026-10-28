#figure(
  image("assets/images/dot.svg", width: 80%),
  caption: [A red dot],
)

Inline #box(image("assets/images/dot.svg", height: 1em)) image and the same file twice.

#link("https://example.com/pic.png")[Remote]

#figure(
  image("assets/images/my dot.svg", width: 80%),
  caption: [Spaced],
)
// resource: images/dot.svg (image/svg+xml, 110 bytes)
// resource: images/my dot.svg (image/svg+xml, 100 bytes)
// warning remote-image ×1: 1 remote image became links (e.g. https://example.com/pic.png).
// warning missing-image ×2: 2 images not found (e.g. images/missing.png).
