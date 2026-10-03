export function Badge(label: string) {
  return (
    <span className="badge">
      <strong className="badge-label">{label}</strong>
      <em className="badge-note">{label}</em>
      <small className="badge-hint">{label}</small>
      <b className="badge-mark">{label}</b>
    </span>
  );
}
