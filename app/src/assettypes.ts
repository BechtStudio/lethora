// Asset types by file extension, the web view's half of the table in
// `crates/typeset/src/assets.rs` (`TYPES`: kinds, extensions, import check):
// how the asset explorer shows an item and what inserting it writes. Only
// images for now; data files (for graphs) can be added.

/** How the explorer previews an item. */
export type PreviewKind = "image";

export interface AssetType {
  /** Shown for the type (list view, tooltips). */
  label: string;
  /** CSS class of the item's icon. */
  icon: string;
  preview: PreviewKind;
  /** Lowercase extensions, as in the Rust table (Typst matches them case-sensitively). */
  extensions: string[];
  /** The largest file accepted, in bytes (`max_bytes` in the Rust table). */
  maxBytes: number;
  /** The Typst code that inserts the asset at `path` (a figure's body for images). */
  snippet(path: string): string;
}

export const ASSET_TYPES = {
  image: {
    label: "Image",
    icon: "asset-icon-image",
    preview: "image",
    extensions: ["png", "jpg", "jpeg", "gif", "svg", "webp"],
    maxBytes: 50 * 1024 * 1024,
    snippet: (path: string) => `image("${path}", width: 80%)`,
  },
} satisfies Record<string, AssetType>;

/** A kind of `ASSET_TYPES` (the `kind` that `asset_list` reports). */
export type AssetKind = keyof typeof ASSET_TYPES;

/** The kind of `path` by its lowercase extension, or null if unknown. */
export function assetKind(path: string): AssetKind | null {
  const m = /\.([a-z0-9]+)$/.exec(path);
  if (!m) return null;
  for (const [kind, type] of Object.entries(ASSET_TYPES) as [AssetKind, AssetType][]) {
    if (type.extensions.includes(m[1])) return kind;
  }
  return null;
}
