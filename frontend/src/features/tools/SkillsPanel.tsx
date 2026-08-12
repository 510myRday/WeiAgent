import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertCircle, ArrowLeft, BookOpen, Braces, ChevronDown, ChevronRight, ChevronUp, Database, Download, FileArchive, FileAudio, FileCode2, FileCog, FileImage, FileJson, FileWarning, Files, FileSpreadsheet, FileTerminal, FileText, FileVideo, FolderOpen, Plus, Presentation, Settings, Sparkles, Trash2 } from 'lucide-react';

import type { SkillConflict, SkillContent, SkillImportTask, SkillManifest, SkillOverrides, SkillSettings } from '../../api';
import { cleanupSkillOrphans, deleteSkill, importSkill, listIpProxies, listSkillImportTasks, listSkills, pickDirectory, readAgentSkillsConfig, readSkillChanges, readSkillContent, updateAgentSkillsConfig, updateSkill, updateSkillEnabled, updateSkillGroupEnabled, updateSkillIcon } from '../../api';
import type { IpProxy } from '../../types';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { Button, Dialog, FormField, IconButton, ListItem, PaneHeader, useFeedback } from '../../components/ui';
import { ProxyPickerDialog } from '../settings/web-search/ProxyPickerDialog';
import { displayProxyUrl } from '../settings/web-search/utils';
import { SkillIcon } from './SkillIcon';

type SkillsPanelProps = {
  onOpenMain: () => void;
  onBackToList: () => void;
  embedded?: boolean;
  embeddedAgentId?: string;
  onOpenIpProxies?: () => void;
};

type ActiveSkill =
  | { kind: 'skill'; name: string }
  | { kind: 'conflict'; name: string }
  | { kind: 'orphan'; scope: 'skill' | 'group'; name: string }
  | { kind: 'group'; name: string };

function displayPath(path: string) {
  const value = path.trim();
  const uncPrefix = '\\\\?\\UNC\\';
  const extendedPrefix = '\\\\?\\';
  if (value.startsWith(uncPrefix)) return `\\\\${value.slice(uncPrefix.length)}`;
  if (value.startsWith(extendedPrefix)) return value.slice(extendedPrefix.length);
  return value;
}

export function SkillsPanel({ onOpenMain, onBackToList, embedded = false, embeddedAgentId, onOpenIpProxies }: SkillsPanelProps) {
  const { t } = useI18n();
  const { confirm, toast } = useFeedback();
  const [settings, setSettings] = useState<SkillSettings | null>(null);
  const [importTasks, setImportTasks] = useState<SkillImportTask[]>([]);
  const [agentSettings, setAgentSettings] = useState<SkillSettings | null>(null);
  const [activeAgentId, setActiveAgentId] = useState(embeddedAgentId ?? '');
  const [active, setActive] = useState<ActiveSkill | null>(null);
  const [content, setContent] = useState<SkillContent | null>(null);
  const [selectedContentPath, setSelectedContentPath] = useState('SKILL.md');
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [importMode, setImportMode] = useState<'local' | 'network'>('local');
  const [importLocalPath, setImportLocalPath] = useState('');
  const [importNetworkUrl, setImportNetworkUrl] = useState('');
  const [importProxyId, setImportProxyId] = useState('');
  const [importProxies, setImportProxies] = useState<IpProxy[]>([]);
  const [importProxiesLoading, setImportProxiesLoading] = useState(false);
  const [importProxyPickerOpen, setImportProxyPickerOpen] = useState(false);
  const [importing, setImporting] = useState(false);
  const [iconOpen, setIconOpen] = useState(false);
  const [iconDraft, setIconDraft] = useState('');
  const [iconSaving, setIconSaving] = useState(false);
  const [skillUpdating, setSkillUpdating] = useState(false);
  const [updateProxyId, setUpdateProxyId] = useState('');
  const [updateProxiesLoading, setUpdateProxiesLoading] = useState(false);
  const [updateProxyPickerOpen, setUpdateProxyPickerOpen] = useState(false);
  const [downloadsOpen, setDownloadsOpen] = useState(false);
  const pendingImportTasks = useRef(new Set<string>());
  const skillChangeVersion = useRef(0);

  const visibleSettings = embedded && activeAgentId && agentSettings ? agentSettings : settings;
  const activeSkill = active?.kind === 'skill'
    ? visibleSettings?.defaults.find((item) => item.name === active.name)
    : undefined;
  const activeConflict = active?.kind === 'conflict'
    ? visibleSettings?.conflicts.find((item) => item.name === active.name)
    : undefined;
  const activeOrphan = active?.kind === 'orphan'
    ? {
        scope: active.scope,
        name: active.name,
        exists: active.scope === 'skill'
          ? visibleSettings?.orphan_skills.includes(active.name) ?? false
          : visibleSettings?.orphan_groups.includes(active.name) ?? false,
      }
    : undefined;
  const activeDownloadTasks = useMemo(
    () => importTasks.filter((task) => task.status === 'queued' || task.status === 'running' || task.status === 'failed'),
    [importTasks],
  );
  const groupedSkills = useMemo(() => {
    const map = new Map<string, SkillManifest[]>();
    for (const skill of visibleSettings?.defaults ?? []) {
      const group = skill.group || t('skills.ungrouped');
      const items = map.get(group) ?? [];
      items.push(skill);
      map.set(group, items);
    }
    return [...map.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [visibleSettings, t]);
  const skillConflicts = visibleSettings?.conflicts ?? [];

  async function loadGlobal() {
    setLoading(true);
    try {
      const next = await listSkills();
      setSettings(next);
      if (!active && next.defaults[0]) setActive({ kind: 'skill', name: next.defaults[0].name });
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('skills.loadFailed'));
    } finally {
      setLoading(false);
    }
  }

  // This initial load intentionally runs once when the panel mounts.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { void loadGlobal(); }, []);

  useEffect(() => {
    let disposed = false;
    async function pollImportTasks() {
      try {
        const next = await listSkillImportTasks();
        if (disposed) return;
        const disappearedPendingTask = [...pendingImportTasks.current].some(
          (taskId) => !next.some((task) => task.id === taskId),
        );
        setImportTasks(next);
        next.forEach((task) => {
          if (task.status === 'queued' || task.status === 'running') {
            pendingImportTasks.current.add(task.id);
          } else {
            pendingImportTasks.current.delete(task.id);
          }
        });
        if (disappearedPendingTask) {
          const refreshed = await listSkills();
          if (!disposed) {
            setSettings(refreshed);
            if (activeSkill?.name) {
              const refreshedContent = await readSkillContent(activeSkill.name);
              if (!disposed) setContent(refreshedContent);
            }
          }
        }
      } catch {
        // The task list is supplementary; the main Skill view remains usable.
      }
    }
    void pollImportTasks();
    const timer = window.setInterval(() => void pollImportTasks(), 1200);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [activeSkill?.name]);

  useEffect(() => {
    if (embeddedAgentId !== undefined) setActiveAgentId(embeddedAgentId);
  }, [embeddedAgentId]);

  useEffect(() => {
    if (!activeAgentId) {
      setAgentSettings(null);
      return;
    }
    void readAgentSkillsConfig(activeAgentId).then(setAgentSettings).catch((error) => {
      toast.error(error instanceof Error ? error.message : t('skills.loadFailed'));
    });
  }, [activeAgentId, t, toast]);

  useEffect(() => {
    const skillName = activeSkill?.name;
    if (!skillName) {
      setContent(null);
      setSelectedContentPath('SKILL.md');
      return;
    }
    void readSkillContent(skillName).then((next) => {
      setContent(next);
      setSelectedContentPath(next.path || 'SKILL.md');
    }).catch(() => setContent(null));
  }, [activeSkill?.name]);

  useEffect(() => {
    let disposed = false;
    let polling = false;

    async function pollSkillChanges() {
      if (disposed || polling) return;
      polling = true;
      try {
        const feed = await readSkillChanges(skillChangeVersion.current);
        if (disposed) return;
        skillChangeVersion.current = feed.version;
        if (!feed.events.length) return;

        const listChanged = feed.full_refresh || feed.events.some((event) => event.list_changed);
        const currentChanged = activeSkill?.name
          ? feed.full_refresh || feed.events.some((event) => event.changed_skills.includes(activeSkill.name))
          : false;

        let nextSettings = settings;
        if (listChanged) {
          nextSettings = await listSkills();
          if (disposed) return;
          setSettings(nextSettings);
        }

        const currentManifest = activeSkill?.name
          ? nextSettings?.defaults.find((item) => item.name === activeSkill.name)
          : undefined;
        if (currentChanged && currentManifest) {
          let nextContent: SkillContent;
          try {
            nextContent = await readSkillContent(currentManifest.name, selectedContentPath);
          } catch {
            nextContent = await readSkillContent(currentManifest.name);
          }
          if (disposed) return;
          setContent(nextContent);
          setSelectedContentPath(nextContent.path || selectedContentPath || 'SKILL.md');
        } else if (activeSkill?.name && !currentManifest) {
          setActive(null);
          setContent(null);
          setSelectedContentPath('SKILL.md');
          onBackToList();
        } else if (active?.kind === 'conflict' && !nextSettings?.conflicts.some((item) => item.name === active.name)) {
          setActive(null);
          onBackToList();
        } else if (active?.kind === 'orphan' && !(active.scope === 'skill'
          ? nextSettings?.orphan_skills.includes(active.name)
          : nextSettings?.orphan_groups.includes(active.name))) {
          setActive(null);
          onBackToList();
        }
      } catch {
        // Automatic refresh is best-effort; the manual refresh remains available.
      } finally {
        polling = false;
      }
    }

    void pollSkillChanges();
    const timer = window.setInterval(() => void pollSkillChanges(), 1500);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [active, activeSkill?.name, onBackToList, selectedContentPath, settings]);

  function selectSkill(name: string) {
    setActive({ kind: 'skill', name });
    onOpenMain();
  }

  function selectConflict(name: string) {
    setActive({ kind: 'conflict', name });
    onOpenMain();
  }

  function selectOrphan(scope: 'skill' | 'group', name: string) {
    setActive({ kind: 'orphan', scope, name });
    onOpenMain();
  }

  async function chooseImportDirectory() {
    try {
      const result = await pickDirectory(importLocalPath);
      if (result.path) setImportLocalPath(result.path);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('skills.importFailed'));
    }
  }

  async function handleImport() {
    const source = (importMode === 'local' ? importLocalPath : importNetworkUrl).trim();
    if (!source) return;
    setImporting(true);
    try {
      const task = await importSkill(
        source,
        importMode === 'network' ? importProxyId || undefined : undefined,
        importMode,
      );
      setImportTasks((current) => [task, ...current.filter((item) => item.id !== task.id)]);
      pendingImportTasks.current.add(task.id);
      setImportOpen(false);
      setImportMode('local');
      setImportLocalPath('');
      setImportNetworkUrl('');
      setImportProxyId('');
      setImportProxyPickerOpen(false);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('skills.importFailed'));
    } finally {
      setImporting(false);
    }
  }

  async function toggleGlobal(name: string, enabled: boolean) {
    setBusy(true);
    try { setSettings(await updateSkillEnabled(name, enabled)); } catch (error) { toast.error(error instanceof Error ? error.message : t('skills.saveFailed')); }
    finally { setBusy(false); }
  }

  async function toggleGroup(group: string, enabled: boolean) {
    setBusy(true);
    try { setSettings(await updateSkillGroupEnabled(group, enabled)); } catch (error) { toast.error(error instanceof Error ? error.message : t('skills.saveFailed')); }
    finally { setBusy(false); }
  }

  async function toggleAgentSkill(skill: SkillManifest, value: boolean | null) {
    if (!activeAgentId || !agentSettings) return;
    const skills = agentSettings.overrides.skills.filter((item) => item.name !== skill.name);
    skills.push({ name: skill.name, enabled: value });
    const next: SkillOverrides = { skills, groups: agentSettings.overrides.groups };
    setBusy(true);
    try { setAgentSettings(await updateAgentSkillsConfig(activeAgentId, next)); } catch (error) { toast.error(error instanceof Error ? error.message : t('skills.saveFailed')); }
    finally { setBusy(false); }
  }

  async function toggleAgentGroup(group: string, value: boolean | null) {
    if (!activeAgentId || !agentSettings) return;
    const groups = agentSettings.overrides.groups.filter((item) => item.group !== group);
    groups.push({ group, enabled: value });
    setBusy(true);
    try { setAgentSettings(await updateAgentSkillsConfig(activeAgentId, { skills: agentSettings.overrides.skills, groups })); } catch (error) { toast.error(error instanceof Error ? error.message : t('skills.saveFailed')); }
    finally { setBusy(false); }
  }

  async function cleanOrphans() {
    const orphanSkills = settings?.orphan_skills ?? [];
    const orphanGroups = settings?.orphan_groups ?? [];
    const accepted = await confirm({
      title: t('skills.cleanOrphansTitle'),
      message: t('skills.cleanOrphansConfirm', {
        skills: orphanSkills.length,
        groups: orphanGroups.length,
      }),
      description: t('skills.cleanOrphansDescription'),
      confirmLabel: t('skills.cleanOrphans'),
      tone: 'warning',
    });
    if (!accepted) return;
    setBusy(true);
    try { setSettings(await cleanupSkillOrphans()); } catch (error) { toast.error(error instanceof Error ? error.message : t('skills.saveFailed')); }
    finally { setBusy(false); }
  }

  async function openImportDialog() {
    setImportOpen(true);
    setImportProxiesLoading(true);
    try {
      setImportProxies(await listIpProxies());
    } catch {
      setImportProxies([]);
    } finally {
      setImportProxiesLoading(false);
    }
  }

  async function openIconDialog() {
    if (!activeSkill) return;
    setIconDraft(activeSkill.icon_url ?? '');
    setUpdateProxyId('');
    setIconOpen(true);
    if (activeSkill.source_url) {
      setUpdateProxiesLoading(true);
      try {
        setImportProxies(await listIpProxies());
      } catch {
        setImportProxies([]);
      } finally {
        setUpdateProxiesLoading(false);
      }
    }
  }

  async function saveIcon() {
    if (!activeSkill) return;
    setIconSaving(true);
    try {
      setSettings(await updateSkillIcon(activeSkill.name, iconDraft.trim() || null));
      setIconOpen(false);
      toast.success(t('skills.iconSaved'));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('skills.iconSaveFailed'));
    } finally {
      setIconSaving(false);
    }
  }

  async function updateRemoteSkill() {
    if (!activeSkill?.source_url) return;
    setSkillUpdating(true);
    try {
      const task = await updateSkill(activeSkill.name, activeSkill.source_url, updateProxyId || undefined);
      setImportTasks((current) => [task, ...current.filter((item) => item.id !== task.id)]);
      pendingImportTasks.current.add(task.id);
      setIconOpen(false);
      toast.success(t('skills.updateQueued'));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('skills.updateFailed'));
    } finally {
      setSkillUpdating(false);
    }
  }

  async function removeActiveSkill() {
    if (!activeSkill) return;
    const name = activeSkill.name;
    const accepted = await confirm({
      title: t('skills.deleteTitle'),
      message: t('skills.deleteConfirm', { name }),
      confirmLabel: t('common.delete'),
      tone: 'danger',
    });
    if (!accepted) return;
    setBusy(true);
    try {
      setSettings(await deleteSkill(name));
      setActive(null);
      setContent(null);
      onBackToList();
      toast.success(t('skills.deleted'));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('skills.deleteFailed'));
    } finally {
      setBusy(false);
    }
  }

  const selectedImportProxy = importProxies.find((proxy) => proxy.id === importProxyId) ?? null;
  const selectedUpdateProxy = importProxies.find((proxy) => proxy.id === updateProxyId) ?? null;
  const importValue = importMode === 'local' ? importLocalPath.trim() : importNetworkUrl.trim();

  return (
    <>
      <PanelShell
        className={`tools-panel skills-panel${embedded ? ' skills-embedded' : ''}`}
      sidebar={(
        <div className="context-pane tools-nav">
          <PaneHeader
            className="mobile-pane-header"
            title={t('skills.title')}
            actions={(
              <>
                {activeDownloadTasks.length > 0 ? <IconButton variant="ghost" type="button" label={t('skills.importTasks')} onClick={() => setDownloadsOpen(true)} disabled={busy}>
                  <Download size={18} />
                </IconButton> : null}
                {!embedded ? (
                  <IconButton variant="ghost" type="button" label={t('skills.add')} onClick={() => void openImportDialog()} disabled={busy || importing}>
                    <Plus size={18} />
                  </IconButton>
                ) : null}
              </>
            )}
          />
          <div className="list-stack">
            {groupedSkills.map(([group, items]) => (
              <div key={group} className="skills-group-list">
                <div className="skills-group-heading">
                  <span>{group}</span>
                  <button type="button" onClick={() => setActive({ kind: 'group', name: group })}>{items.length}</button>
                </div>
                {items.map((skill) => (
                  <ListItem key={skill.name} className="skills-list-item" type="button" active={active?.kind === 'skill' && active.name === skill.name} title={<span className="tool-list-title"><span>{skill.name}</span><span className={`mini-status-badge ${skill.enabled ? 'skill-enabled' : 'skill-disabled'}`}><BookOpen size={11} />{skill.enabled ? t('skills.enabled') : t('skills.disabled')}</span></span>} subtitle={<span className="skills-list-description" title={skill.description}>{skill.description}</span>} leading={<SkillIcon iconUrl={skill.icon_url} size={19} />} onClick={() => selectSkill(skill.name)} />
                ))}
              </div>
            ))}
            {skillConflicts.length > 0 ? (
              <div className="skills-conflict-list">
                <div className="skills-group-heading skills-conflict-heading">
                  <span>{t('skills.conflicts')}</span>
                  <span className="mini-status-badge skills-conflict-badge"><AlertCircle size={11} />{skillConflicts.length}</span>
                </div>
                {skillConflicts.map((conflict) => (
                  <ListItem
                    key={`conflict:${conflict.name}`}
                    className="skills-list-item skills-conflict-item"
                    type="button"
                    active={active?.kind === 'conflict' && active.name === conflict.name}
                    title={<span className="tool-list-title"><span>{conflict.name}</span><span className="mini-status-badge skills-conflict-badge"><AlertCircle size={11} />{t('skills.nameConflict')}</span></span>}
                    subtitle={<span className="skills-list-description">{t('skills.nameConflictCount', { count: conflict.count })}</span>}
                    leading={<AlertCircle size={19} />}
                    onClick={() => selectConflict(conflict.name)}
                  />
                ))}
              </div>
            ) : null}
            {settings && (settings.orphan_skills.length > 0 || settings.orphan_groups.length > 0) ? (
              <div className="skills-orphan-list">
                <div className="skills-group-heading skills-conflict-heading">
                  <span>{t('skills.orphanCategory')}</span>
                  <span className="mini-status-badge skills-conflict-badge"><AlertCircle size={11} />{settings.orphan_skills.length + settings.orphan_groups.length}</span>
                </div>
                {settings.orphan_skills.map((name) => (
                  <ListItem
                    key={`orphan-skill:${name}`}
                    className="skills-list-item skills-orphan-item"
                    type="button"
                    active={active?.kind === 'orphan' && active.scope === 'skill' && active.name === name}
                    title={<span className="tool-list-title"><span>{name}</span><span className="mini-status-badge skills-conflict-badge"><AlertCircle size={11} />{t('skills.orphanBadge')}</span></span>}
                    subtitle={<span className="skills-list-description">{settings.conflicts.some((item) => item.name === name) ? t('skills.orphanConflict') : t('skills.orphanMissing')}</span>}
                    leading={<AlertCircle size={19} />}
                    onClick={() => selectOrphan('skill', name)}
                  />
                ))}
                {settings.orphan_groups.map((name) => (
                  <ListItem
                    key={`orphan-group:${name}`}
                    className="skills-list-item skills-orphan-item"
                    type="button"
                    active={active?.kind === 'orphan' && active.scope === 'group' && active.name === name}
                    title={<span className="tool-list-title"><span>{name}</span><span className="mini-status-badge skills-conflict-badge"><AlertCircle size={11} />{t('skills.orphanBadge')}</span></span>}
                    subtitle={<span className="skills-list-description">{t('skills.orphanGroup')}</span>}
                    leading={<AlertCircle size={19} />}
                    onClick={() => selectOrphan('group', name)}
                  />
                ))}
              </div>
            ) : null}
            {!loading && settings && !settings.defaults.length && !settings.conflicts.length ? (
              <div className="skills-empty-state">
                <strong>{t('skills.empty')}</strong>
                <span>{t('skills.emptyPath', { path: displayPath(settings.skills_dir) })}</span>
              </div>
            ) : null}
          </div>
        </div>
      )}
      main={(
        <div className="main-pane config-main">
          {embedded ? (
            <div className="agent-tools-overview skills-agent-overview">
              <div className="agent-tools-overview__header">
                <div>
                  <h3>{t('skills.agentTitle')}</h3>
                  <p>{t('skills.agentDescription')}</p>
                </div>
              </div>
              <AgentSkillsOverview settings={visibleSettings} busy={busy} onToggle={toggleAgentSkill} />
            </div>
          ) : (
            <>
              <PaneHeader className="config-page-header" leading={<Button className="main-back-button" variant="ghost" type="button" onClick={onBackToList} icon={<ArrowLeft size={17} />}>{t('common.back')}</Button>} eyebrow={t('skills.title')} title={activeOrphan ? activeOrphan.name : (activeConflict?.name ?? activeSkill?.name ?? t('skills.title'))} description={activeOrphan ? t('skills.orphanHeaderDescription') : (activeConflict ? t('skills.nameConflictDescription', { name: activeConflict.name, count: activeConflict.count }) : (activeSkill?.description ?? t('skills.description')))} actions={(
                <>
                  {activeSkill ? <IconButton variant="default" type="button" label={t('skills.iconSettings')} onClick={openIconDialog} disabled={busy || iconSaving}><Settings size={17} /></IconButton> : null}
                  {activeSkill ? <IconButton variant="danger" type="button" label={t('skills.delete')} onClick={() => void removeActiveSkill()} disabled={busy}><Trash2 size={17} /></IconButton> : null}
                  {activeSkill ? <SkillToggle skill={activeSkill} agentMode={false} busy={busy} onGlobalToggle={toggleGlobal} onAgentToggle={toggleAgentSkill} /> : null}
                </>
              )} />
              {activeOrphan?.exists ? <SkillOrphanDetail orphan={activeOrphan} onClean={() => void cleanOrphans()} busy={busy} /> : null}
              {activeConflict ? <SkillConflictDetail conflict={activeConflict} /> : null}
              {active?.kind === 'group' ? <GroupEditor group={active.name} settings={visibleSettings} agentMode={false} busy={busy} onGlobalToggle={toggleGroup} onAgentToggle={toggleAgentGroup} /> : null}
              {activeSkill && content ? <SkillEditor skill={activeSkill} content={content} selectedPath={selectedContentPath} onPathChange={setSelectedContentPath} /> : null}
            </>
          )}
        </div>
      )}
      />
      {downloadsOpen ? (
        <Dialog
          open={downloadsOpen}
          title={t('skills.importTasks')}
          description={t('skills.importTasksDescription')}
          icon={<Download size={21} />}
          closeLabel={t('common.close')}
          onClose={() => setDownloadsOpen(false)}
          size="md"
          bodyClassName="skills-import-tasks--dialog"
        >
          <div className="skills-import-tasks">
            {activeDownloadTasks.map((task) => (
              <div key={task.id} className={`skills-import-task is-${task.status}`}>
                <div className="skills-import-task__title">
                  <strong>{task.kind === 'network' ? t('skills.networkTab') : t('skills.localTab')}</strong>
                  <span>{task.status === 'failed' ? t('skills.importFailedShort') : t(`skills.importPhase.${task.phase}`)}</span>
                </div>
                <code title={task.source}>{task.source}</code>
                {task.status === 'queued' || task.status === 'running' ? <div className="skills-import-task__progress"><span style={{ width: `${task.progress}%` }} /></div> : null}
                {task.error ? <small>{task.error}</small> : null}
              </div>
            ))}
          </div>
        </Dialog>
      ) : null}
      {iconOpen ? (
        <Dialog
          open={iconOpen}
          title={t('skills.iconSettings')}
          description={t('skills.iconSettingsDescription')}
          icon={<Settings size={21} />}
          closeLabel={t('common.close')}
          onClose={() => { if (!iconSaving) setIconOpen(false); }}
          size="sm"
          footer={(
            <>
              <Button variant="ghost" type="button" onClick={() => setIconOpen(false)} disabled={iconSaving}>
                {t('common.cancel')}
              </Button>
              <Button variant="primary" type="button" onClick={() => void saveIcon()} disabled={iconSaving}>
                {t('common.save')}
              </Button>
            </>
          )}
        >
          <div className="skills-icon-editor">
            <FormField label={t('skills.iconUrlLabel')} htmlFor="skill-icon-url" hint={t('skills.iconUrlHint')}>
              <input
                id="skill-icon-url"
                type="text"
                value={iconDraft}
                placeholder={t('skills.iconUrlPlaceholder')}
                onChange={(event) => setIconDraft(event.target.value)}
              />
            </FormField>
            {activeSkill?.source_url ? (
              <section className="skills-source-settings">
                <div className="skills-source-settings__heading">
                  <strong>{t('skills.sourceUrlLabel')}</strong>
                  <span>{t('skills.sourceUrlHint')}</span>
                </div>
                <code className="skills-source-settings__url" title={activeSkill.source_url}>{activeSkill.source_url}</code>
                <label className="form-field">
                  <span>{t('skills.proxyLabel')}</span>
                  <button
                    type="button"
                    className={`web-search-proxy-picker${selectedUpdateProxy ? '' : ' is-empty'}`}
                    onClick={() => setUpdateProxyPickerOpen(true)}
                    disabled={skillUpdating}
                  >
                    <span className="web-search-proxy-picker__copy">
                      {selectedUpdateProxy ? (
                        <>
                          <strong>{selectedUpdateProxy.name}</strong>
                          <code>{displayProxyUrl(selectedUpdateProxy.url)}</code>
                        </>
                      ) : (
                        <span>{t('skills.selectProxy')}</span>
                      )}
                    </span>
                    <ChevronDown size={17} aria-hidden="true" />
                  </button>
                  {selectedUpdateProxy && (!selectedUpdateProxy.enabled || selectedUpdateProxy.status !== 'available') ? (
                    <span className="form-field-hint"><AlertCircle size={14} />{t('skills.proxyUnavailable')}</span>
                  ) : null}
                </label>
                <p className="skills-source-settings__proxy-hint">{t('skills.updateProxyHint')}</p>
                <Button
                  variant="secondary"
                  type="button"
                  onClick={() => void updateRemoteSkill()}
                  disabled={iconSaving || skillUpdating}
                  icon={<Download size={15} />}
                >
                  {skillUpdating ? t('skills.updating') : t('skills.update')}
                </Button>
              </section>
            ) : null}
          </div>
        </Dialog>
      ) : null}
      {!embedded ? (
        <Dialog
          open={importOpen}
          title={t('skills.importTitle')}
          description={t('skills.importDescription')}
          icon={<FolderOpen size={21} />}
          closeLabel={t('common.close')}
          onClose={() => { if (!importing) setImportOpen(false); }}
          size="sm"
          footer={(
            <>
              <Button variant="ghost" type="button" onClick={() => setImportOpen(false)} disabled={importing}>{t('common.cancel')}</Button>
              <Button variant="primary" type="button" onClick={() => void handleImport()} disabled={!importValue || importing} icon={<Plus size={16} />}>
                {t('skills.import')}
              </Button>
            </>
          )}
        >
          <div className="skills-import-picker">
            <div className="skills-import-tabs" role="tablist" aria-label={t('skills.importModeLabel')}>
              <button
                type="button"
                role="tab"
                aria-selected={importMode === 'local'}
                className={importMode === 'local' ? 'is-active' : ''}
                onClick={() => setImportMode('local')}
              >
                <FolderOpen size={15} />{t('skills.localTab')}
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={importMode === 'network'}
                className={importMode === 'network' ? 'is-active' : ''}
                onClick={() => setImportMode('network')}
              >
                <Sparkles size={15} />{t('skills.networkTab')}
              </button>
            </div>
            {importMode === 'local' ? (
              <div className="skills-import-panel" role="tabpanel">
                <label className="form-field">
                  <span>{t('skills.localPathLabel')}</span>
                  <input
                    type="text"
                    value={importLocalPath}
                    onChange={(event) => setImportLocalPath(event.target.value)}
                    placeholder={t('skills.localPathPlaceholder')}
                    disabled={importing}
                  />
                </label>
                <Button variant="secondary" type="button" onClick={() => void chooseImportDirectory()} disabled={importing} icon={<FolderOpen size={16} />}>
                  {t('skills.chooseDirectory')}
                </Button>
                <p>{importLocalPath || t('skills.noLocalPath')}</p>
              </div>
            ) : (
              <div className="skills-import-panel" role="tabpanel">
                <label className="form-field">
                  <span>{t('skills.networkUrlLabel')}</span>
                  <input
                    type="url"
                    value={importNetworkUrl}
                    onChange={(event) => setImportNetworkUrl(event.target.value)}
                    placeholder={t('skills.networkUrlPlaceholder')}
                    disabled={importing}
                    spellCheck={false}
                  />
                </label>
                <label className="form-field">
                  <span>{t('skills.proxyLabel')}</span>
                  <button
                    type="button"
                    className={`web-search-proxy-picker${selectedImportProxy ? '' : ' is-empty'}`}
                    onClick={() => setImportProxyPickerOpen(true)}
                    disabled={importing}
                  >
                    <span className="web-search-proxy-picker__copy">
                      {selectedImportProxy ? (
                        <>
                          <strong>{selectedImportProxy.name}</strong>
                          <code>{displayProxyUrl(selectedImportProxy.url)}</code>
                        </>
                      ) : (
                        <span>{t('skills.selectProxy')}</span>
                      )}
                    </span>
                    <ChevronDown size={17} aria-hidden="true" />
                  </button>
                  {selectedImportProxy && (!selectedImportProxy.enabled || selectedImportProxy.status !== 'available') ? (
                    <span className="form-field-hint"><AlertCircle size={14} />{t('skills.proxyUnavailable')}</span>
                  ) : null}
                </label>
                <p>{t('skills.networkHint')}</p>
              </div>
            )}
          </div>
        </Dialog>
      ) : null}
      {importProxyPickerOpen ? (
        <ProxyPickerDialog
          value={selectedImportProxy?.url ?? null}
          ipProxies={importProxies}
          isLoading={importProxiesLoading}
          onSelect={(url) => {
            const selected = url ? importProxies.find((proxy) => proxy.url === url) : null;
            setImportProxyId(selected?.id ?? '');
            setImportProxyPickerOpen(false);
          }}
          onClose={() => setImportProxyPickerOpen(false)}
          onOpenIpProxies={onOpenIpProxies ? () => {
            setImportProxyPickerOpen(false);
            setImportOpen(false);
            onOpenIpProxies();
          } : undefined}
        />
      ) : null}
      {updateProxyPickerOpen ? (
        <ProxyPickerDialog
          value={selectedUpdateProxy?.url ?? null}
          ipProxies={importProxies}
          isLoading={updateProxiesLoading}
          onSelect={(url) => {
            const selected = url ? importProxies.find((proxy) => proxy.url === url) : null;
            setUpdateProxyId(selected?.id ?? '');
            setUpdateProxyPickerOpen(false);
          }}
          onClose={() => setUpdateProxyPickerOpen(false)}
          onOpenIpProxies={onOpenIpProxies ? () => {
            setUpdateProxyPickerOpen(false);
            setIconOpen(false);
            onOpenIpProxies();
          } : undefined}
        />
      ) : null}
    </>
  );
}

function SkillConflictDetail({ conflict }: { conflict: SkillConflict }) {
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

function SkillOrphanDetail({ orphan, onClean, busy }: { orphan: { scope: 'skill' | 'group'; name: string }; onClean: () => void; busy: boolean }) {
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

function AgentSkillsOverview({ settings, busy, onToggle }: { settings: SkillSettings | null; busy: boolean; onToggle: (skill: SkillManifest, value: boolean | null) => Promise<void> }) {
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
              <label className="inherit-control" title={t('skills.inheritDescription')}>
                <input
                  type="checkbox"
                  aria-label={`${skill.name} ${t('tools.inheritsDefault')}`}
                  checked={inherited}
                  disabled={busy}
                  onChange={(event) => void onToggle(skill, event.target.checked ? null : skill.enabled)}
                />
                <span>{t('tools.inheritsDefault')}</span>
              </label>
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

function SkillDescription({ description }: { description: string }) {
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

function SkillEditor({ skill, content, selectedPath, onPathChange }: { skill: SkillManifest; content: SkillContent; selectedPath: string; onPathChange: (path: string) => void }) {
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
        <span className="skills-updated-at">{t('skills.updatedAt')}: {formatSkillTimestamp(content.updated_at)}</span>
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

function SkillToggle({ skill, agentMode, busy, onGlobalToggle, onAgentToggle }: { skill: SkillManifest; agentMode: boolean; busy: boolean; onGlobalToggle: (name: string, enabled: boolean) => Promise<void>; onAgentToggle: (skill: SkillManifest, value: boolean | null) => Promise<void> }) {
  const { t } = useI18n();
  const inherited = agentMode && skill.agent_override == null && skill.agent_group_override == null;
  return (
    <label className={`toggle-switch${inherited ? ' is-disabled' : ''}`} title={skill.enabled ? t('skills.enabled') : t('skills.disabled')}>
      <input aria-label={`${skill.name} ${skill.enabled ? t('skills.enabled') : t('skills.disabled')}`} type="checkbox" checked={skill.enabled} disabled={busy || inherited} onChange={(event) => void (agentMode ? onAgentToggle(skill, event.target.checked) : onGlobalToggle(skill.name, event.target.checked))} />
      <span className="toggle-slider" />
    </label>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const rounded = value >= 100 ? Math.round(value) : Math.round(value * 10) / 10;
  return `${rounded} ${units[unitIndex]}`;
}

function formatSkillTimestamp(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'medium' }).format(date);
}

function GroupEditor({ group, settings, agentMode, busy, onGlobalToggle, onAgentToggle }: { group: string; settings: SkillSettings | null; agentMode: boolean; busy: boolean; onGlobalToggle: (group: string, enabled: boolean) => Promise<void>; onAgentToggle: (group: string, value: boolean | null) => Promise<void> }) {
  const { t } = useI18n();
  const state = settings?.groups.find((item) => item.name === group);
  if (!state) return null;
  const override = agentMode ? state.agent_override : state.global_override;
  return <section className="form-card skills-summary-card"><div><h2>{group}</h2><p>{t('skills.groupHint')}</p></div><label className={`toggle-switch${agentMode && override == null ? ' is-disabled' : ''}`}><input aria-label={`${group} ${state.enabled ? t('skills.enabled') : t('skills.disabled')}`} type="checkbox" checked={state.enabled} disabled={busy || (agentMode && override == null)} onChange={(event) => void (agentMode ? onAgentToggle(group, event.target.checked) : onGlobalToggle(group, event.target.checked))} /><span className="toggle-slider" /></label><span className="skills-inheritance-label">{agentMode ? (override == null ? t('skills.inherited') : t('skills.override')) : t('skills.globalDefault')}</span></section>;
}
