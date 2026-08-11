import { useState } from 'react';
import { ChevronDown, ChevronUp } from 'lucide-react';

import { useI18n } from '../../i18n';

const DESCRIPTION_COLLAPSE_LENGTH = 180;

export function McpToolDetails({ description, inputSchema }: { description: string; inputSchema: unknown }) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const collapsible = description.length > DESCRIPTION_COLLAPSE_LENGTH;

  return (
    <div className="mcp-tool-details">
      <div className={`mcp-tool-details__description${collapsible && !expanded ? ' is-collapsed' : ''}`}>
        <p>{description}</p>
        {collapsible ? (
          <button
            type="button"
            className="mcp-tool-details__description-toggle"
            aria-expanded={expanded}
            onClick={() => setExpanded((value) => !value)}
          >
            {expanded ? <ChevronUp size={15} aria-hidden="true" /> : <ChevronDown size={15} aria-hidden="true" />}
            <span>{expanded ? t('tools.mcp.collapseDescription') : t('tools.mcp.expandDescription')}</span>
          </button>
        ) : null}
      </div>
      <details className="mcp-tool-details__schema">
        <summary>{t('tools.mcp.showSchema')}</summary>
        <pre>{JSON.stringify(inputSchema, null, 2)}</pre>
      </details>
    </div>
  );
}
