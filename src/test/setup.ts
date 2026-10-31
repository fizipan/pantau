import "@testing-library/jest-dom/vitest";

// jsdom has no matchMedia, which syncColorScheme needs.
window.matchMedia ??= (query: string) =>
  ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
  }) as unknown as MediaQueryList;
