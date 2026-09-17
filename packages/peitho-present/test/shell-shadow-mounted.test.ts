import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SHADOW_MOUNTED_EVENT, mountPresentShell } from "../src/index";
import type { PresentShell } from "../src/index";

// Covers the peitho:shadow-mounted bridge (see shell.ts): a CustomEvent per host, plus the
// window-level backlog a mounting script that hasn't loaded yet can drain once it does.

function okJson(value: unknown): Response {
  return { ok: true, status: 200, json: async () => value } as Response;
}

function okText(value: string): Response {
  return { ok: true, status: 200, text: async () => value } as Response;
}

const manifest = {
  version: 1,
  peithoVersion: "0.1.0",
  title: "Demo",
  slideCount: 2,
  plannedDurationMs: null,
  aspectRatio: "16:9",
  canvasWidth: 1280,
  canvasHeight: 720,
  sections: [],
  slides: [
    {
      index: 0,
      key: "intro",
      src: "slides/000-intro.html",
      hasNotes: false,
      skip: false,
      revealSteps: 0,
      text: { title: "", body: "", code: "" }
    },
    {
      index: 1,
      key: "arch-1",
      src: "slides/001-arch-1.html",
      hasNotes: false,
      skip: false,
      revealSteps: 0,
      text: { title: "", body: "", code: "" }
    }
  ]
};
const cssText = ".slot-title { color: rebeccapurple; }";

const mountedShells: PresentShell[] = [];

function standardFetch(): typeof fetch {
  return vi.fn(async (url: string) => {
    if (url === "manifest.json") return okJson(manifest);
    if (url === "peitho.css") return okText(cssText);
    if (url === "slides/000-intro.html") return okText('<section><div data-bf="Foo"></div></section>');
    if (url === "slides/001-arch-1.html") return okText('<section><div data-bf="Bar"></div></section>');
    return { ok: false, status: 404, text: async () => "not found", json: async () => ({}) } as Response;
  }) as unknown as typeof fetch;
}

beforeEach(() => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
    (() => null) as HTMLCanvasElement["getContext"]
  );
  delete (window as unknown as Record<string, unknown>).__peithoShadowRoots;
});

afterEach(() => {
  while (mountedShells.length > 0) {
    mountedShells.pop()?.destroy();
  }
  document.body.innerHTML = "";
  delete (window as unknown as Record<string, unknown>).__peithoShadowRoots;
  vi.restoreAllMocks();
});

async function mountForTest(root: HTMLElement): Promise<PresentShell> {
  const shell = await mountPresentShell({ root, fetcher: standardFetch() });
  mountedShells.push(shell);
  return shell;
}

it("spec: registers every mounted shadow root in window.__peithoShadowRoots, in order", async () => {
  const root = document.createElement("main");
  document.body.appendChild(root);

  await mountForTest(root);

  const hosts = root.querySelectorAll<HTMLElement>(".peitho-slide");
  const registry = (window as unknown as Record<string, ShadowRoot[]>).__peithoShadowRoots;
  expect(registry).toHaveLength(2);
  expect(registry[0]).toBe(hosts[0].shadowRoot);
  expect(registry[1]).toBe(hosts[1].shadowRoot);
});

it("spec: dispatches peitho:shadow-mounted for every host, reaching document via composed:true", async () => {
  const root = document.createElement("main");
  document.body.appendChild(root);
  const seenRoots: unknown[] = [];
  const listener = (event: Event): void => {
    seenRoots.push((event as CustomEvent<{ root?: ShadowRoot }>).detail?.root);
  };
  document.addEventListener(SHADOW_MOUNTED_EVENT, listener);

  try {
    await mountForTest(root);
  } finally {
    document.removeEventListener(SHADOW_MOUNTED_EVENT, listener);
  }

  const hosts = root.querySelectorAll<HTMLElement>(".peitho-slide");
  expect(seenRoots).toEqual([hosts[0].shadowRoot, hosts[1].shadowRoot]);
});

it("adversarial: a shell whose root never joins the document still fills the backlog (no throw)", async () => {
  const root = document.createElement("main");

  await mountForTest(root);

  const registry = (window as unknown as Record<string, ShadowRoot[]>).__peithoShadowRoots;
  expect(registry).toHaveLength(2);
});

it("adversarial: two shells sharing one document both append to the same backlog, not overwrite it", async () => {
  const firstRoot = document.createElement("main");
  const secondRoot = document.createElement("main");
  document.body.appendChild(firstRoot);
  document.body.appendChild(secondRoot);

  await mountForTest(firstRoot);
  await mountForTest(secondRoot);

  const registry = (window as unknown as Record<string, ShadowRoot[]>).__peithoShadowRoots;
  expect(registry).toHaveLength(4);
});
