const DATE_TIME_FORMATTER = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});
const BYTES_FORMATTER = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 1,
});
const BYTE_UNITS = ["B", "KiB", "MiB", "GiB", "TiB"] as const;

/**
 * A date and time in the viewer's locale, such as "Oct 2, 2026, 3:04 PM".
 *
 * `value` is an ISO timestamp, epoch milliseconds or a `Date`. A missing
 * value yields `empty`; one that is not a date is shown as it was received.
 */
export function formatDateTime(
  value: string | number | Date | null | undefined,
  empty = "Never",
): string {
  if (value === null || value === undefined || value === "") return empty;
  const date = value instanceof Date ? value : new Date(value);
  return Number.isNaN(date.getTime())
    ? String(value)
    : DATE_TIME_FORMATTER.format(date);
}

/**
 * A byte count in binary units with at most one decimal place, such as
 * "1.5 KiB" or "12 GiB".
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${BYTES_FORMATTER.format(value)} ${BYTE_UNITS[unit]}`;
}
