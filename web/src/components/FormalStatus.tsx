import type { FormalCodeStatus } from "../lib/types";
const labels = {
  unverified: "Unverified",
  sorry: "Sorry",
  conditional: "Conditional",
  verified: "Verified",
};
export function FormalStatus({
  language,
  status,
  compact = false,
}: {
  language: "Lean" | "Rocq";
  status: FormalCodeStatus;
  compact?: boolean;
}) {
  const label = `${language}: ${labels[status]}`;
  return (
    <span
      className="formal-status"
      data-status={status}
      title={label}
      aria-label={label}
    >
      <span className="status-light" aria-hidden="true" />
      <span className="formal-language">{language}</span>
      {!compact && <span className="status-text">{labels[status]}</span>}
    </span>
  );
}
