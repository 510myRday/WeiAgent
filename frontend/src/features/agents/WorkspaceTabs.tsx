import type { KeyboardEvent } from 'react';

export type WorkspaceField = 'personality' | 'memory' | 'instructions';

interface WorkspaceTabsProps {
  fields: readonly WorkspaceField[];
  activeField: WorkspaceField;
  label: string;
  panelId: string;
  getLabel: (field: WorkspaceField) => string;
  onChange: (field: WorkspaceField) => void;
}

export function WorkspaceTabs({
  fields,
  activeField,
  label,
  panelId,
  getLabel,
  onChange,
}: WorkspaceTabsProps) {
  function handleKeyDown(event: KeyboardEvent<HTMLButtonElement>, field: WorkspaceField) {
    const currentIndex = fields.indexOf(field);
    let nextIndex: number | null = null;
    if (event.key === 'ArrowRight') nextIndex = (currentIndex + 1) % fields.length;
    if (event.key === 'ArrowLeft') nextIndex = (currentIndex - 1 + fields.length) % fields.length;
    if (event.key === 'Home') nextIndex = 0;
    if (event.key === 'End') nextIndex = fields.length - 1;
    if (nextIndex === null) return;

    event.preventDefault();
    const nextField = fields[nextIndex];
    onChange(nextField);
    event.currentTarget.parentElement
      ?.querySelector<HTMLElement>(`#agent-workspace-tab-${nextField}`)
      ?.focus();
  }

  return (
    <div className="workspace-tabs" role="tablist" aria-label={label}>
      {fields.map((field) => {
        const active = activeField === field;
        return (
          <button
            key={field}
            id={`agent-workspace-tab-${field}`}
            type="button"
            className={`workspace-tab${active ? ' active' : ''}`}
            role="tab"
            tabIndex={active ? 0 : -1}
            aria-selected={active}
            aria-controls={panelId}
            onClick={() => onChange(field)}
            onKeyDown={(event) => handleKeyDown(event, field)}
          >
            {getLabel(field)}
          </button>
        );
      })}
    </div>
  );
}
