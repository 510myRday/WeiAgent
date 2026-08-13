import { useEffect, useMemo, useState } from 'react';
import {
  AlertCircle,
  BookOpen,
  Braces,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  Database,
  FileArchive,
  FileAudio,
  FileCode2,
  FileCog,
  FileImage,
  FileJson,
  FileWarning,
  Files,
  FileSpreadsheet,
  FileTerminal,
  FileText,
  FileVideo,
  FolderOpen,
  Presentation,
  Trash2,
} from 'lucide-react';

import type { SkillConflict, SkillContent, SkillManifest, SkillSettings } from '../../../toolTypes';
import { readSkillContent } from '../../../api';
import { formatBytes, formatDateTime } from '../../../utils/format';
import { useI18n } from '../../../i18n';
import { Button, InheritToggle } from '../../../components/ui';
import { displayPath } from './displayPath';

export function SkillConflictDetail({ conflict }: { conflict: SkillConflict }) {
  const { t } = useI18n();
  return (
    <section className="form-card skills-conflict-detail">
      <div className="skills-conflict-detail__header">
        <AlertCircle size={22} aria-hidden="true" />
        <div>
          <h2>{t('skills.nameConflictTitle')}</h2>
          <p>{t('skills.nameConflictDetail', { name: conflict.name, count: conflict.count })}</p>
        </div>
      </div>
      <div className="skills-conflict-detail__summary">
        <div><span>{t('skills.nameConflictStatusLabel')}</span><strong>{t('skills.nameConflictStatus')}</strong></div>
        <div><span>{t('skills.nameConflictCountLabel')}</span><strong>{conflict.count}</strong></div>
      </div>
      <div className="skills-conflict-detail__notice">{t('skills.nameConflictAction')}</div>
      <div className="skills-conflict-detail__paths">
        <strong>{t('skills.nameConflictPaths')}</strong>
        {conflict.paths.map((path) => <code key={path}><FolderOpen size={14} />{displayPath(path)}</code>)}
      </div>
    </section>
  );
}

export function SkillOrphanDetail({ orphan, onClean, busy }: { orphan: { scope: 'skill' | 'group'; name: string }; onClean: () => void; busy: boolean }) {
  const { t } = useI18n();
  const reason = orphan.scope === 'group' ? t('skills.orphanGroup') : t('skills.orphanMissing');
  return (
    <section className="form-card skills-orphan-detail">
      <div className="skills-orphan-detail__header">
        <AlertCircle size={22} aria-hidden="true" />
        <div>
          <h2>{t('skills.orphanTitle')}</h2>
          <p>{t('skills.orphanDetail', { name: orphan.name, reason })}</p>
        </div>
      </div>
      <div className="skills-orphan-detail__summary">
        <div><span>{t('skills.orphanStatusLabel')}</span><strong>{t('skills.orphanStatus')}</strong></div>
        <div><span>{t('skills.orphanReasonLabel')}</span><strong>{reason}</strong></div>
      </div>
      <div className="skills-orphan-detail__notice">{t('skills.orphanAction')}</div>
      <Button variant="secondary" type="button" onClick={onClean} disabled={busy} icon={<Trash2 size={15} />}>
        {t('skills.cleanOrphans')}
      </Button>
    </section>
  );
}

export function AgentSkillsOverview({ settings, busy, onToggle }: { settings: SkillSettings | null; busy: boolean; onToggle: (skill: SkillManifest, value: boolean | null) => Promise<void> }) {
  const { t } = useI18n();
  if (!settings) return <div className="skills-loading-state">{t('chat.loading')}</div>;
  if (settings.defaults.length === 0) return <div className="agent-tool-empty">{t('skills.empty')}</div>;
  return (
    <div className="agent-tool-list skills-agent-list">
      {settings.defaults.map((skill) => {
        const inherited = skill.agent_override == null && skill.agent_group_override == null;
        return (
          <div className="agent-tool-row skills-agent-row" key={skill.name}>
            <div className="agent-tool-row__body">
              <div className="agent-tool-row__title">
                <strong>{skill.name}</strong>
                <span className="mini-status-badge skill-badge"><BookOpen size={12} aria-hidden="true" />{t('skills.title')}</span>
              </div>
              <SkillDescription description={skill.description || t('skills.noDescription')} />
              <span className="agent-tool-row__source">{inherited ? t('tools.inheritsDefault') : t('tools.agentOverride')}</span>
            </div>
            <div className="agent-tool-row__actions skills-agent-row__actions">
              <InheritToggle
                inherited={inherited}
                disabled={busy}
                title={t('skills.inheritDescription')}
                ariaLabel={`${skill.name} ${t('tools.inheritsDefault')}`}
                onChange={(value) => void onToggle(skill, value ? null : skill.enabled)}
                label={t('tools.inheritsDefault')}
              />
              <label className={`toggle-switch${inherited ? ' is-disabled' : ''}`} title={skill.enabled ? t('skills.enabled') : t('skills.disabled')}>
                <input
                  type="checkbox"
                  aria-label={`${skill.name} ${skill.enabled ? t('skills.enabled') : t('skills.disabled')}`}
                  checked={skill.enabled}
                  disabled={busy || inherited}
                  onChange={(event) => void onToggle(skill, event.target.checked)}
                />
                <span className="toggle-slider" />
              </label>
            </div>
          </div>
        );
      })}
    </div>
  );
}

const DESCRIPTION_COLLAPSE_LENGTH = 180;
const MAX_SKILL_FILE_BYTES = 5 * 1024 * 1024;

export function SkillDescription({ description }: { description: string }) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const collapsible = description.length > DESCRIPTION_COLLAPSE_LENGTH;

  return (
    <div className={`skills-row-description${collapsible && !expanded ? ' is-collapsed' : ''}`}>
      <small>{description}</small>
      {collapsible ? (
        <button
          type="button"
          className="skills-row-description__toggle"
          aria-expanded={expanded}
          onClick={() => setExpanded((value) => !value)}
        >
          {expanded ? <ChevronUp size={15} aria-hidden="true" /> : <ChevronDown size={15} aria-hidden="true" />}
          <span>{expanded ? t('tools.mcp.collapseDescription') : t('tools.mcp.expandDescription')}</span>
        </button>
      ) : null}
    </div>
  );
}

export function SkillEditor({ skill, content, selectedPath, onPathChange }: { skill: SkillManifest; content: SkillContent; selectedPath: string; onPathChange: (path: string) => void }) {
  const { t } = useI18n();
  const [preview, setPreview] = useState(content.content);
  const [previewMediaType, setPreviewMediaType] = useState(content.media_type);
  const [previewData, setPreviewData] = useState(content.data_base64 ?? null);
  const [previewNotice, setPreviewNotice] = useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const files = useMemo(() => content.files.length > 0 ? content.files : [
    { path: content.path || 'SKILL.md', kind: 'skill_md', size_bytes: content.content.length },
    ...content.references,
    ...content.examples,
    ...content.scripts,
  ], [content.content, content.examples, content.files, content.path, content.references, content.scripts]);
  const tree = useMemo(() => buildSkillFileTree(files), [files]);

  useEffect(() => {
    onPathChange(content.path || 'SKILL.md');
    setPreview(content.content);
    setPreviewMediaType(content.media_type);
    setPreviewData(content.data_base64 ?? null);
    setPreviewNotice(null);
  }, [content.path, content.content, content.media_type, content.data_base64, onPathChange]);

  async function selectFile(path: string) {
    onPathChange(path);
    const file = files.find((item) => item.path === path);
    if (file && file.size_bytes > MAX_SKILL_FILE_BYTES) {
      setPreview('');
      setPreviewMediaType('text/plain');
      setPreviewData(null);
      setPreviewNotice(t('skills.fileTooLarge', {
        size: formatBytes(file.size_bytes),
        limit: formatBytes(MAX_SKILL_FILE_BYTES),
      }));
      setPreviewLoading(false);
      return;
    }
    setPreviewNotice(null);
    if (path === content.path || path === 'SKILL.md') {
      setPreview(content.content);
      setPreviewMediaType(content.media_type);
      setPreviewData(content.data_base64 ?? null);
      return;
    }
    setPreviewLoading(true);
    try {
      const next = await readSkillContent(skill.name, path);
      setPreview(next.content);
      setPreviewMediaType(next.media_type);
      setPreviewData(next.data_base64 ?? null);
    } catch {
      setPreview('');
      setPreviewMediaType('text/plain');
      setPreviewData(null);
    } finally {
      setPreviewLoading(false);
    }
  }

  return (
    <section className="skills-detail-stack">
      <div className="skills-file-manager">
        <div className="skills-file-manager__body" aria-label={t('skills.fileCount', { count: files.length })}>
          <div className="skills-file-tree">
            <div className="skills-file-tree__root" title={skill.name}>
              <FolderOpen size={15} />
              <span>{skill.name}</span>
              <b className="skills-file-tree__count">{files.length}</b>
            </div>
            <SkillFileTree nodes={tree} selectedPath={selectedPath} onSelect={(path) => void selectFile(path)} />
          </div>
          <div className="skills-file-preview">
            <div className="skills-file-preview__header"><strong>{selectedPath}</strong>{previewLoading ? <span>{t('chat.loading')}</span> : null}</div>
            {previewMediaType.startsWith('image/') && previewData ? (
              <div className="skills-file-preview__media"><img src={`data:${previewMediaType};base64,${previewData}`} alt={selectedPath} /></div>
            ) : previewNotice ? (
              <div className="skills-file-preview__placeholder">
                <FileWarning size={22} aria-hidden="true" />
                <p>{previewNotice}</p>
              </div>
            ) : <pre>{preview}</pre>}
          </div>
          </div>
      </div>
      <div className="skills-detail-meta">
        <p className="skills-cwd" title={displayPath(content.cwd)}><FolderOpen size={14} /><span>{displayPath(content.cwd)}</span></p>
        <span className="skills-updated-at">{t('skills.updatedAt')}: {formatDateTime(content.updated_at)}</span>
      </div>
    </section>
  );
}

type SkillFileTreeNode = {
  name: string;
  path: string;
  file?: SkillContent['files'][number];
  children: SkillFileTreeNode[];
};

function buildSkillFileTree(files: SkillContent['files']): SkillFileTreeNode[] {
  const roots: SkillFileTreeNode[] = [];
  for (const file of files) {
    const parts = file.path.split('/').filter(Boolean);
    let current = roots;
    let parentPath = '';
    parts.forEach((part, index) => {
      const path = parentPath ? `${parentPath}/${part}` : part;
      let node = current.find((item) => item.name === part);
      if (!node) {
        node = { name: part, path, children: [] };
        current.push(node);
      }
      if (index === parts.length - 1) node.file = file;
      current = node.children;
      parentPath = path;
    });
  }
  const sortNodes = (nodes: SkillFileTreeNode[]) => {
    nodes.sort((a, b) => Number(Boolean(b.children.length)) - Number(Boolean(a.children.length)) || a.name.localeCompare(b.name));
    nodes.forEach((node) => sortNodes(node.children));
  };
  sortNodes(roots);
  return roots;
}

function SkillFileTree({ nodes, selectedPath, onSelect, depth = 0 }: { nodes: SkillFileTreeNode[]; selectedPath: string; onSelect: (path: string) => void; depth?: number }) {
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  return <div className="skills-file-tree__nodes">{nodes.map((node) => {
    const isDirectory = node.children.length > 0;
    const isOpen = expanded[node.path] ?? true;
    const FileIcon = node.file ? skillFileIcon(node.file.path, node.file.kind) : FolderOpen;
    return <div key={node.path} className="skills-file-tree__node">
      <button
        type="button"
        className={`skills-file-row${node.file && selectedPath === node.file.path ? ' is-active' : ''}${isDirectory ? ' is-directory' : ''}`}
        style={{ paddingLeft: `${10 + depth * 16}px` }}
        onClick={() => {
          if (isDirectory) setExpanded((current) => ({ ...current, [node.path]: !isOpen }));
          if (node.file) onSelect(node.file.path);
        }}
        title={node.file?.path ?? node.path}
      >
        {isDirectory ? (isOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />) : <span className="skills-file-row__spacer" />}
        <FileIcon size={15} />
        <span>{node.name}</span>
        {node.file ? <small>{formatBytes(node.file.size_bytes)}</small> : <small className="skills-file-row__count">{countTreeFiles(node)}</small>}
      </button>
      {isDirectory && isOpen ? <SkillFileTree nodes={node.children} selectedPath={selectedPath} onSelect={onSelect} depth={depth + 1} /> : null}
    </div>;
  })}</div>;
}

function countTreeFiles(node: SkillFileTreeNode): number {
  return (node.file ? 1 : 0) + node.children.reduce((total, child) => total + countTreeFiles(child), 0);
}

function skillFileIcon(path: string, kind: string) {
  const normalizedPath = path.toLowerCase();
  const fileName = normalizedPath.split('/').pop() ?? normalizedPath;

  if (kind === 'skill_md' || /\.md$/i.test(normalizedPath)) return BookOpen;
  if (/\.(png|jpe?g|gif|webp|bmp|svg|ico|avif|heic)$/i.test(normalizedPath)) return FileImage;
  if (/\.(json|json5|jsonl)$/i.test(normalizedPath)) return FileJson;
  if (/\.(ya?ml|toml|xml|ini|conf|config|cfg|properties|env)$/i.test(normalizedPath) || fileName.startsWith('.env')) return FileCog;
  if (/\.(csv|tsv|xls|xlsx|xlsm|ods)$/i.test(normalizedPath)) return FileSpreadsheet;
  if (/\.(ppt|pptx|pptm|odp)$/i.test(normalizedPath)) return Presentation;
  if (/\.(doc|docx|docm|odt|rtf|txt|pdf)$/i.test(normalizedPath)) return FileText;
  if (/\.(zip|rar|7z|tar|gz|bz2|xz|zst|tgz)$/i.test(normalizedPath)) return FileArchive;
  if (/\.(mp3|wav|flac|ogg|m4a|aac|opus)$/i.test(normalizedPath)) return FileAudio;
  if (/\.(mp4|mkv|avi|mov|webm|m4v|wmv)$/i.test(normalizedPath)) return FileVideo;
  if (/\.(db|sqlite|sqlite3|sql|mdb|accdb)$/i.test(normalizedPath)) return Database;
  if (/\.(sh|bash|zsh|fish|ps1|psm1|bat|cmd|com)$/i.test(normalizedPath) || /^(dockerfile|makefile|justfile)$/.test(fileName)) return FileTerminal;
  if (/\.(py|pyw|js|jsx|mjs|cjs|ts|tsx|rs|go|java|kt|kts|c|h|cc|cpp|cxx|hpp|cs|php|rb|swift|dart|lua|r|scala|ex|exs|erl|hrl|clj|groovy|vue|svelte|css|scss|less|html|htm|wasm)$/i.test(normalizedPath)) return FileCode2;
  if (/^(package-lock\.json|pnpm-lock\.yaml|yarn\.lock|cargo\.lock|go\.sum)$/.test(fileName)) return Braces;
  return Files;
}

export function SkillToggle({ skill, agentMode, busy, onGlobalToggle, onAgentToggle }: { skill: SkillManifest; agentMode: boolean; busy: boolean; onGlobalToggle: (name: string, enabled: boolean) => Promise<void>; onAgentToggle: (skill: SkillManifest, value: boolean | null) => Promise<void> }) {
  const { t } = useI18n();
  const inherited = agentMode && skill.agent_override == null && skill.agent_group_override == null;
  return (
    <label className={`toggle-switch${inherited ? ' is-disabled' : ''}`} title={skill.enabled ? t('skills.enabled') : t('skills.disabled')}>
      <input aria-label={`${skill.name} ${skill.enabled ? t('skills.enabled') : t('skills.disabled')}`} type="checkbox" checked={skill.enabled} disabled={busy || inherited} onChange={(event) => void (agentMode ? onAgentToggle(skill, event.target.checked) : onGlobalToggle(skill.name, event.target.checked))} />
      <span className="toggle-slider" />
    </label>
  );
}

export function GroupEditor({ group, settings, agentMode, busy, onGlobalToggle, onAgentToggle }: { group: string; settings: SkillSettings | null; agentMode: boolean; busy: boolean; onGlobalToggle: (group: string, enabled: boolean) => Promise<void>; onAgentToggle: (group: string, value: boolean | null) => Promise<void> }) {
  const { t } = useI18n();
  const state = settings?.groups.find((item) => item.name === group);
  if (!state) return null;
  const override = agentMode ? state.agent_override : state.global_override;
  return <section className="form-card skills-summary-card"><div><h2>{group}</h2><p>{t('skills.groupHint')}</p></div><label className={`toggle-switch${agentMode && override == null ? ' is-disabled' : ''}`}><input aria-label={`${group} ${state.enabled ? t('skills.enabled') : t('skills.disabled')}`} type="checkbox" checked={state.enabled} disabled={busy || (agentMode && override == null)} onChange={(event) => void (agentMode ? onAgentToggle(group, event.target.checked) : onGlobalToggle(group, event.target.checked))} /><span className="toggle-slider" /></label><span className="skills-inheritance-label">{agentMode ? (override == null ? t('skills.inherited') : t('skills.override')) : t('skills.globalDefault')}</span></section>;
}
