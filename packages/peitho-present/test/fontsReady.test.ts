import { afterEach, expect, it, vi } from "vitest";
import { waitForFontsReady } from "../src/fontsReady";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function deferred<T>(): { promise: Promise<T>; resolve: (value: T | PromiseLike<T>) => void } {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((innerResolve) => {
    resolve = innerResolve;
  });
  return { promise, resolve };
}

async function flushMicrotasks(count = 10): Promise<void> {
  for (let index = 0; index < count; index += 1) {
    await Promise.resolve();
  }
}

type FontFaceStub = {
  family: string;
  weight?: string;
  style?: string;
};

/** The text every test passes; without it nothing is kicked (see waitForFontsReady). */
const SAMPLE_TEXT = "Hello";

function face(family: string): FontFaceStub {
  return { family };
}

function defineFonts(
  doc: Document,
  {
    status,
    ready,
    faces = [],
    load = vi.fn(async () => [])
  }: {
    status: FontFaceSet["status"];
    ready: Promise<unknown> | (() => Promise<unknown>);
    faces?: FontFaceStub[] | (() => FontFaceStub[]);
    load?: (font: string, text?: string) => unknown;
  }
): void {
  Object.defineProperty(doc, "fonts", {
    configurable: true,
    value: {
      status,
      load,
      get ready(): Promise<unknown> {
        return typeof ready === "function" ? ready() : ready;
      },
      forEach: (callback: (face: FontFace) => void) => {
        const visibleFaces = typeof faces === "function" ? faces() : faces;
        for (const stub of visibleFaces) {
          callback({ weight: "400", style: "normal", ...stub } as unknown as FontFace);
        }
      }
    }
  });
}

it("resolves immediately without registering a timer when doc.fonts is undefined", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  Object.defineProperty(doc, "fonts", { configurable: true, value: undefined });
  const setTimeoutSpy = vi.spyOn(window, "setTimeout");

  await expect(waitForFontsReady(doc, window)).resolves.toBeUndefined();

  expect(setTimeoutSpy).not.toHaveBeenCalled();
});

it("waits until doc.fonts.ready resolves when fonts are still loading", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  const ready = deferred<void>();
  defineFonts(doc, { status: "loading", ready: ready.promise });
  const clearTimeoutSpy = vi.spyOn(window, "clearTimeout");
  let resolved = false;

  const waiting = waitForFontsReady(doc, window).then(() => {
    resolved = true;
  });
  await Promise.resolve();

  expect(resolved).toBe(false);

  ready.resolve();
  await waiting;

  expect(resolved).toBe(true);
  expect(clearTimeoutSpy).toHaveBeenCalledTimes(1);
});

it("resolves after the default timeout and warns when doc.fonts.ready never resolves", async () => {
  vi.useFakeTimers();
  const doc = document.implementation.createHTMLDocument("test");
  defineFonts(doc, { status: "loading", ready: new Promise<void>(() => {}) });
  const win = {
    setTimeout: window.setTimeout.bind(window),
    clearTimeout: window.clearTimeout.bind(window),
    console: window.console
  } as unknown as Window;
  const log = { warn: vi.fn() };
  let resolved = false;

  const waiting = waitForFontsReady(doc, win, { log }).then(() => {
    resolved = true;
  });
  await Promise.resolve();

  expect(resolved).toBe(false);

  await vi.advanceTimersByTimeAsync(3000);
  await waiting;

  expect(resolved).toBe(true);
  expect(log.warn).toHaveBeenCalledTimes(1);
  expect(log.warn.mock.calls[0]?.[0]).toContain("timed out");
  expect(log.warn.mock.calls[0]?.[0]).toContain("3000ms");
});

it("respects a custom timeoutMs", async () => {
  vi.useFakeTimers();
  const doc = document.implementation.createHTMLDocument("test");
  defineFonts(doc, { status: "loading", ready: new Promise<void>(() => {}) });
  const win = {
    setTimeout: window.setTimeout.bind(window),
    clearTimeout: window.clearTimeout.bind(window),
    console: window.console
  } as unknown as Window;
  const log = { warn: vi.fn() };
  let resolved = false;

  const waiting = waitForFontsReady(doc, win, { timeoutMs: 500, log }).then(() => {
    resolved = true;
  });
  await Promise.resolve();

  await vi.advanceTimersByTimeAsync(400);
  expect(resolved).toBe(false);
  expect(log.warn).not.toHaveBeenCalled();

  await vi.advanceTimersByTimeAsync(100);
  await Promise.resolve();

  expect(resolved).toBe(true);
  await waiting;
  expect(log.warn).toHaveBeenCalledTimes(1);
  expect(log.warn.mock.calls[0]?.[0]).toContain("500ms");
});

it("kicks the deck's text through fonts.load() before awaiting ready", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  const load = vi.fn(async () => []);
  let readyResolved = false;
  defineFonts(doc, {
    status: "loading",
    ready: Promise.resolve().then(() => {
      readyResolved = true;
    }),
    faces: [face("Inter"), face("Noto Sans JP")],
    load
  });

  const waiting = waitForFontsReady(doc, window, { text: SAMPLE_TEXT });

  expect(load).toHaveBeenCalledTimes(2);
  expect(load).toHaveBeenCalledWith("normal 400 1em Inter", SAMPLE_TEXT);
  expect(load).toHaveBeenCalledWith("normal 400 1em Noto Sans JP", SAMPLE_TEXT);
  expect(readyResolved).toBe(false);

  await waiting;
});

it("asks once per family+weight+style, so subsets never fan out into a request each", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  const load = vi.fn(async () => []);
  // What a subsetted CJK family looks like: one family, many unicode-range faces.
  const subsets = Array.from({ length: 124 }, () => face("Noto Sans JP"));
  defineFonts(doc, {
    status: "loading",
    ready: Promise.resolve(),
    faces: [...subsets, { family: "Inter", weight: "700", style: "normal" }],
    load
  });

  await expect(waitForFontsReady(doc, window, { text: SAMPLE_TEXT })).resolves.toBeUndefined();

  expect(load).toHaveBeenCalledTimes(2);
  expect(load).toHaveBeenCalledWith("normal 400 1em Noto Sans JP", SAMPLE_TEXT);
  expect(load).toHaveBeenCalledWith("normal 700 1em Inter", SAMPLE_TEXT);
});

it("kicks nothing when the deck has no text to match", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  const load = vi.fn(async () => []);
  defineFonts(doc, {
    status: "loading",
    ready: Promise.resolve(),
    faces: [face("Inter")],
    load
  });

  await expect(waitForFontsReady(doc, window, { text: "" })).resolves.toBeUndefined();

  expect(load).not.toHaveBeenCalled();
});

it("kicks fonts.load() even while the set already reports loaded", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  const load = vi.fn(async () => []);
  defineFonts(doc, {
    status: "loaded",
    ready: Promise.resolve(),
    faces: [face("Inter")],
    load
  });

  await expect(waitForFontsReady(doc, window, { text: SAMPLE_TEXT })).resolves.toBeUndefined();

  expect(load).toHaveBeenCalledTimes(1);
});

it("swallows synchronous throws from fonts.load()", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  const load = vi.fn(() => {
    throw new Error("invalid");
  });
  defineFonts(doc, {
    status: "loading",
    ready: Promise.resolve(),
    faces: [face("Inter")],
    load
  });

  await expect(waitForFontsReady(doc, window, { text: SAMPLE_TEXT })).resolves.toBeUndefined();

  expect(load).toHaveBeenCalledTimes(1);
});

it("swallows rejections from fonts.load()", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  const load = vi.fn(async () => Promise.reject(new Error("net")));
  defineFonts(doc, {
    status: "loading",
    ready: Promise.resolve(),
    faces: [face("Inter")],
    load
  });

  await expect(waitForFontsReady(doc, window, { text: SAMPLE_TEXT })).resolves.toBeUndefined();

  expect(load).toHaveBeenCalledTimes(1);
});

it("resolves without warning when fonts are already loaded", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  defineFonts(doc, { status: "loaded", ready: Promise.resolve() });
  const log = { warn: vi.fn() };

  await expect(
    waitForFontsReady(doc, window, { log, text: SAMPLE_TEXT })
  ).resolves.toBeUndefined();

  expect(log.warn).not.toHaveBeenCalled();
});

it("picks up faces that appear after the first pass", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  const firstReady = deferred<void>();
  const secondReady = deferred<void>();
  const load = vi.fn(async () => []);
  const readyPromises = [firstReady.promise, secondReady.promise];
  let readyAccesses = 0;
  let forEachCalls = 0;
  const log = { warn: vi.fn() };
  defineFonts(doc, {
    status: "loaded",
    ready: () => readyPromises[readyAccesses++] ?? Promise.resolve(),
    faces: () => {
      forEachCalls += 1;
      return forEachCalls === 1 ? [] : [face("Inter"), face("Noto Sans JP")];
    },
    load
  });

  const waiting = waitForFontsReady(doc, window, { log, text: SAMPLE_TEXT });
  await Promise.resolve();

  expect(load).not.toHaveBeenCalled();

  firstReady.resolve();
  await flushMicrotasks();

  expect(load).toHaveBeenCalledTimes(2);

  secondReady.resolve();
  await waiting;

  expect(readyAccesses).toBe(2);
  expect(forEachCalls).toBe(3);
  expect(log.warn).not.toHaveBeenCalled();
});

it("stops after a post-ready pass with no new faces", async () => {
  const doc = document.implementation.createHTMLDocument("test");
  let readyAccesses = 0;
  let forEachCalls = 0;
  const log = { warn: vi.fn() };
  defineFonts(doc, {
    status: "loaded",
    ready: () => {
      readyAccesses += 1;
      return Promise.resolve();
    },
    faces: () => {
      forEachCalls += 1;
      return [];
    }
  });

  await expect(
    waitForFontsReady(doc, window, { log, text: SAMPLE_TEXT })
  ).resolves.toBeUndefined();

  expect(readyAccesses).toBe(1);
  expect(forEachCalls).toBe(2);
  expect(log.warn).not.toHaveBeenCalled();
});
