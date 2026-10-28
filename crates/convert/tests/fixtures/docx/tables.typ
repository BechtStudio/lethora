#figure(
  table(
    columns: 3,
    table.header([Name], table.cell(colspan: 2)[Value]),
    table.cell(rowspan: 2)[a], [1], [2],
    [3], [4],
    [b], [cell with

    n1

    n2

    nested table], [],
  ),
  caption: [Results],
)

Between.

#table(
  columns: 2,
  [plain], [table],
  [], [shifted],
)
// lang: en-GB
// warning nested-table ×1: Flattened 1 nested table into the cells' paragraphs.
