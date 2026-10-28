// Read-only parts of a shared project (the text and its settings for
// reviewers and readers, the review for readers): keeps the write controls
// disabled, also ones the panels render later, and enables again exactly
// those it disabled. The core refuses any change that slips through.

/** Write controls: those under `root` matching `selector`. */
export interface WriteControls {
  root: HTMLElement;
  selector: string;
}

type Control = HTMLButtonElement | HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement;

export class WriteGuard {
  private targets: readonly WriteControls[];
  private observers: MutationObserver[] = [];
  private disabled = new Set<Control>();

  constructor(targets: readonly WriteControls[]) {
    this.targets = targets;
  }

  /** Disables the write controls (`on`), or gives them back. */
  set(on: boolean): void {
    for (const o of this.observers) o.disconnect();
    this.observers = [];
    if (!on) {
      for (const c of this.disabled) c.disabled = false;
      this.disabled.clear();
      return;
    }
    for (const t of this.targets) {
      const apply = () => {
        for (const c of t.root.querySelectorAll<Control>(t.selector)) {
          if (c.disabled) continue;
          c.disabled = true;
          this.disabled.add(c);
        }
      };
      apply();
      // Controls rendered later, or enabled again by their panel.
      const observer = new MutationObserver(apply);
      observer.observe(t.root, { childList: true, subtree: true, attributes: true, attributeFilter: ["disabled"] });
      this.observers.push(observer);
    }
  }
}
