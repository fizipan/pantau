import { expect, test } from "vitest";
import { formatAge, formatBytes } from "./format";

test("formatBytes uses a decimal comma and one digit", () => {
  expect(formatBytes(1_181_116_006)).toBe("1,1 GB");
  expect(formatBytes(471 * 1024 * 1024)).toBe("471 MB");
});

test("formatBytes handles the small and the empty cases", () => {
  expect(formatBytes(0)).toBe("0 B");
  expect(formatBytes(512)).toBe("512 B");
  expect(formatBytes(1536)).toBe("1,5 KB");
  expect(formatBytes(2 * 1024 ** 4)).toBe("2,0 TB");
});

test("formatBytes never shows a number that belongs to the next unit", () => {
  expect(formatBytes(1024 * 1024 - 1)).toBe("1,0 MB");
  expect(formatBytes(1024 ** 3 - 1)).toBe("1,0 GB");
  expect(formatBytes(102_399)).toBe("100 KB");
  expect(formatBytes(102_400)).toBe("100 KB");
  expect(formatBytes(1023 * 1024 + 600)).toBe("1,0 MB");
});

test("formatAge says how long ago", () => {
  expect(formatAge(0, 12 * 60_000)).toBe("12 menit lalu");
  expect(formatAge(0, 30_000)).toBe("baru saja");
  expect(formatAge(0, 3 * 3_600_000)).toBe("3 jam lalu");
  expect(formatAge(0, 2 * 86_400_000)).toBe("2 hari lalu");
  expect(formatAge(5_000, 1_000)).toBe("baru saja");
});
