import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertCircle, ArrowLeft, BookOpen, ChevronDown, Download, FolderOpen, Plus, Settings, Sparkles, Trash2 } from 'lucide-react';

import type { SkillContent, SkillImportTask, SkillManifest, SkillOverrides, SkillSettings } from '../../toolTypes';
import { cleanupSkillOrphans, deleteSkill, importSkill, listIpProxies, listSkillImportTasks, listSkills, pickDirectory, readAgentSkillsConfig, readSkillChanges, readSkillContent, updateAgentSkillsConfig, updateSkill, updateSkillEnabled, updateSkillGroupEnabled, updateSkillIcon } from '../../api';
import type { IpProxy } from '../../types';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { Button, Dialog, FormField, IconButton, ListItem, PaneHeader, useFeedback } from '../../components/ui';
import { ProxyPickerDialog } from '../settings/web-search/ProxyPickerDialog';
import { displayProxyUrl } from '../../utils/proxy';
import { SkillIcon } from './SkillIcon';
import { displayPath } from './skills/displayPath';
import { AgentSkillsOverview, GroupEditor, SkillConflictDetail, SkillEditor, SkillOrphanDetail, SkillToggle } from './skills/SkillViews';

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
                  <span className="skills-download-icon" aria-hidden="true">
                    <Download size={18} />
                    <span className="skills-download-icon__badge" />
                  </span>
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



