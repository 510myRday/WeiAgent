interface InheritToggleProps {
  inherited: boolean;
  onChange: (inherited: boolean) => void;
  label: string;
  disabled?: boolean;
  title?: string;
  ariaLabel?: string;
}

export function InheritToggle({ inherited, onChange, label, disabled, title, ariaLabel }: InheritToggleProps) {
  return (
    <label className="inherit-control" title={title}>
      <input
        type="checkbox"
        checked={inherited}
        disabled={disabled}
        aria-label={ariaLabel}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>{label}</span>
    </label>
  );
}
