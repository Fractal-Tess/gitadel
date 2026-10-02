import assert from "node:assert/strict";
import { describe, test } from "node:test";

import { formatBytes, formatDateTime } from "../src/lib/format.js";

// The formatters follow the machine's locale, so expectations are built with
// the same locale rather than spelled out for one of them.
function number(value: number): string {
  return new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 }).format(
    value,
  );
}

describe("formatBytes", () => {
  test("shows whole bytes below one kibibyte", () => {
    assert.equal(formatBytes(0), "0 B");
    assert.equal(formatBytes(1), "1 B");
    assert.equal(formatBytes(1023), `${number(1023)} B`);
  });

  test("steps through binary units with at most one decimal", () => {
    assert.equal(formatBytes(1024), "1 KiB");
    assert.equal(formatBytes(1536), `${number(1.5)} KiB`);
    assert.equal(formatBytes(1024 ** 2), "1 MiB");
    assert.equal(formatBytes(15.72 * 1024 ** 3), `${number(15.7)} GiB`);
    assert.equal(formatBytes(3 * 1024 ** 4), "3 TiB");
  });

  test("stays in the largest unit beyond it", () => {
    assert.equal(formatBytes(2048 * 1024 ** 4), `${number(2048)} TiB`);
  });

  test("treats negative and non-numeric counts as empty", () => {
    assert.equal(formatBytes(-5), "0 B");
    assert.equal(formatBytes(Number.NaN), "0 B");
  });
});

describe("formatDateTime", () => {
  const expected = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date("2026-10-02T15:04:05Z"));

  test("formats ISO strings, epoch milliseconds and dates alike", () => {
    assert.equal(formatDateTime("2026-10-02T15:04:05Z"), expected);
    assert.equal(formatDateTime(Date.UTC(2026, 9, 2, 15, 4, 5)), expected);
    assert.equal(formatDateTime(new Date("2026-10-02T15:04:05Z")), expected);
  });

  test("names a missing value", () => {
    assert.equal(formatDateTime(null), "Never");
    assert.equal(formatDateTime(undefined), "Never");
    assert.equal(formatDateTime("", "Never used"), "Never used");
  });

  test("shows a value that is not a date as it was received", () => {
    assert.equal(formatDateTime("soon"), "soon");
  });
});
