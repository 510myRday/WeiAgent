import { useEffect, useMemo, useRef, useState, type MouseEvent } from 'react';
import { createPortal } from 'react-dom';
import { AlertCircle, ArrowLeft, ChevronDown, ChevronRight, Download, FolderOpen, FolderInput, MoreHorizontal, Pencil, Plus, Settings, Sparkles, Trash2, FolderPlus, CheckSquare, X } from 'lucide-react';

import type { SkillContent, SkillGroupState, SkillImportTask, SkillManifest, SkillOverrides, SkillSettings } from '../../toolTypes';
import { assignSkillGroup, cleanupSkillOrphans, createSkillGroup, deleteSkill, deleteSkillGroup, importSkill, listIpProxies, listSkillImportTasks, listSkills, pickDirectory, readAgentSkillsConfig, readSkillChanges, readSkillContent, renameSkillGroup, reorderSkillGroupItems, reorderSkillGroups, updateAgentSkillsConfig, updateSkillEnabled, updateSkillIcon } from '../../api';
import type { IpProxy } from '../../types';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { Button, Dialog, FormField, IconButton, ListItem, PaneHeader, useFeedback } from '../../components/ui';
import { ProxyPickerDialog } from '../settings/web-search/ProxyPickerDialog';
import { displayProxyUrl } from '../../utils/proxy';
import { SkillIcon } from './SkillIcon';
import { displayPath, displaySkillFilePath } from './skills/displayPath';
import { AgentSkillsOverview, SkillConflictDetail, SkillEditor, SkillOrphanDetail, SkillToggle } from './skills/SkillViews';

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
  | { kind: 'orphan'; name: string };

function isBuiltinGroup(group: SkillGroupState | null | undefined) {
  return Boolean(group?.is_builtin || group?.id === 'builtin');
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
  const [importGroupId, setImportGroupId] = useState('');
  const [importProxies, setImportProxies] = useState<IpProxy[]>([]);
  const [importProxiesLoading, setImportProxiesLoading] = useState(false);
  const [importProxyPickerOpen, setImportProxyPickerOpen] = useState(false);
  const [importing, setImporting] = useState(false);
  const [iconOpen, setIconOpen] = useState(false);
  const [iconDraft, setIconDraft] = useState('');
  const [iconSaving, setIconSaving] = useState(false);
  const [downloadsOpen, setDownloadsOpen] = useState(false);
  const [groupDialog, setGroupDialog] = useState<{ mode: 'create' | 'rename'; group?: SkillGroupState } | null>(null);
  const [moveSkillDialog, setMoveSkillDialog] = useState<{ name: string; groupId: string } | null>(null);
  const [groupDraft, setGroupDraft] = useState('');
  const [groupMenu, setGroupMenu] = useState<string | null>(null);
  const [groupMenuPosition, setGroupMenuPosition] = useState<{ top: number; left: number } | null>(null);
  const [selectedSkills, setSelectedSkills] = useState<string[]>([]);
  const [moveMenuOpen, setMoveMenuOpen] = useState(false);
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>(() => {
    try { return JSON.parse(localStorage.getItem('weiagent-skills-collapsed-groups') ?? '{}') as Record<string, boolean>; } catch { return {}; }
  });
  const [draggedGroupId, setDraggedGroupId] = useState<string | null>(null);
  const [draggedSkillName, setDraggedSkillName] = useState<string | null>(null);
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
        name: active.name,
        exists: visibleSettings?.orphan_skills.includes(active.name) ?? false,
      }
    : undefined;
  const activeDownloadTasks = useMemo(
    () => importTasks.filter((task) => task.status === 'queued' || task.status === 'running' || task.status === 'failed'),
    [importTasks],
  );
  const groupedSkills = useMemo(() => {
    const skills = visibleSettings?.defaults ?? [];
    const groups = [...(visibleSettings?.groups ?? [])]
      .sort((left, right) => {
        const leftBuiltin = isBuiltinGroup(left);
        const rightBuiltin = isBuiltinGroup(right);
        if (leftBuiltin !== rightBuiltin) return leftBuiltin ? -1 : 1;
        return left.sort_order - right.sort_order;
      })
      .map((group) => ({
      group,
      items: skills.filter((skill) => skill.group_id === group.id),
      }));
    const ungrouped = skills.filter((skill) => !skill.group_id);
    return [
      ...groups.filter(({ group }) => isBuiltinGroup(group)),
      { group: null as SkillGroupState | null, items: ungrouped },
      ...groups.filter(({ group }) => !isBuiltinGroup(group)),
    ];
  }, [visibleSettings]);

  function collapseKey(group: SkillGroupState | null) { return group?.id ?? 'ungrouped'; }

  function toggleCollapsed(group: SkillGroupState | null) {
    const key = collapseKey(group);
    setCollapsedGroups((current) => {
      const next = { ...current, [key]: !current[key] };
      try { localStorage.setItem('weiagent-skills-collapsed-groups', JSON.stringify(next)); } catch { /* localStorage unavailable */ }
      return next;
    });
  }

  function openCreateGroup() { setGroupDraft(''); setGroupDialog({ mode: 'create' }); }
  function openRenameGroup(group: SkillGroupState) { setGroupDraft(group.name); setGroupDialog({ mode: 'rename', group }); setGroupMenu(null); setGroupMenuPosition(null); }

  function openGroupMenu(event: MouseEvent<HTMLButtonElement>, groupId: string) {
    const rect = event.currentTarget.getBoundingClientRect();
    const menuWidth = 160;
    const menuHeight = 84;
    const left = Math.max(8, Math.min(rect.right - menuWidth, window.innerWidth - menuWidth - 8));
    const top = rect.bottom + menuHeight + 8 <= window.innerHeight ? rect.bottom + 4 : rect.top - menuHeight - 4;
    setGroupMenu(groupMenu === groupId ? null : groupId);
    setGroupMenuPosition(groupMenu === groupId ? null : { top, left });
  }

  useEffect(() => {
    if (!groupMenu) return undefined;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      const target = event.target as HTMLElement | null;
      if (!target?.closest('.skills-group-menu, .skills-group-menu-wrap')) {
        setGroupMenu(null);
        setGroupMenuPosition(null);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setGroupMenu(null);
        setGroupMenuPosition(null);
      }
    };
    const closeOnScroll = () => {
      setGroupMenu(null);
      setGroupMenuPosition(null);
    };
    document.addEventListener('pointerdown', closeOnOutsidePointer, true);
    window.addEventListener('keydown', closeOnEscape);
    window.addEventListener('scroll', closeOnScroll, true);
    return () => {
      document.removeEventListener('pointerdown', closeOnOutsidePointer, true);
      window.removeEventListener('keydown', closeOnEscape);
      window.removeEventListener('scroll', closeOnScroll, true);
    };
  }, [groupMenu]);

  async function saveGroup() {
    const name = groupDraft.trim();
    if (!name) return;
    setBusy(true);
    try {
      const next = groupDialog?.mode === 'create'
        ? await createSkillGroup(name)
        : await renameSkillGroup(groupDialog?.group?.id ?? '', name);
      setSettings(next);
      setGroupDialog(null);
      toast.success(t('skills.groupSaved'));
    } catch (error) { toast.error(error instanceof Error ? error.message : t('skills.groupSaveFailed')); }
    finally { setBusy(false); }
  }

  async function removeGroup(group: SkillGroupState) {
    const accepted = await confirm({ title: t('skills.deleteGroupTitle'), message: t('skills.deleteGroupConfirm', { name: group.name, count: group.skill_count }), confirmLabel: t('common.delete'), tone: 'danger' });
    if (!accepted) return;
    setBusy(true);
    try { setSettings(await deleteSkillGroup(group.id)); setGroupMenu(null); setGroupMenuPosition(null); toast.success(t('skills.groupDeleted')); }
    catch (error) { toast.error(error instanceof Error ? error.message : t('skills.groupDeleteFailed')); }
    finally { setBusy(false); }
  }

  async function moveSkills(groupId: string | null, names = selectedSkills) {
    if (
      !names.length
      || embedded
      || groupId === 'builtin'
      || visibleSettings?.groups.some((group) => group.id === groupId && isBuiltinGroup(group))
    ) return;
    const movableNames = names.filter((name) => !visibleSettings?.defaults.some((skill) => skill.name === name && skill.is_builtin));
    if (!movableNames.length) return;
    setBusy(true);
    try {
      let next = visibleSettings;
      for (const name of movableNames) next = await assignSkillGroup(name, groupId);
      if (embedded) setAgentSettings(next); else setSettings(next);
      setSelectedSkills([]);
      setMoveMenuOpen(false);
      toast.success(t('skills.skillsMoved', { count: movableNames.length }));
    } catch (error) { toast.error(error instanceof Error ? error.message : t('skills.moveFailed')); }
    finally { setBusy(false); }
  }

  function openMoveSkillDialog(skill: SkillManifest) {
    setMoveSkillDialog({ name: skill.name, groupId: skill.group_id ?? '' });
  }

  async function saveMoveSkill() {
    if (!moveSkillDialog) return;
    const skill = visibleSettings?.defaults.find((item) => item.name === moveSkillDialog.name);
    const targetGroup = visibleSettings?.groups.find((group) => group.id === moveSkillDialog.groupId);
    if (embedded || skill?.is_builtin || isBuiltinGroup(targetGroup)) return;
    setBusy(true);
    try {
      const next = await assignSkillGroup(moveSkillDialog.name, moveSkillDialog.groupId || null);
      if (embedded) setAgentSettings(next); else setSettings(next);
      setMoveSkillDialog(null);
      toast.success(t('skills.skillMoved'));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('skills.moveFailed'));
    } finally {
      setBusy(false);
    }
  }

  function toggleSkillSelection(name: string) {
    setSelectedSkills((current) => current.includes(name) ? current.filter((item) => item !== name) : [...current, name]);
  }

  async function dropGroup(targetId: string) {
    if (!draggedGroupId || draggedGroupId === targetId || embedded || !settings) return;
    const ids = settings.groups.map((group) => group.id);
    if (settings.groups.some((group) => group.id === draggedGroupId && isBuiltinGroup(group)) || settings.groups.some((group) => group.id === targetId && isBuiltinGroup(group))) return;
    const from = ids.indexOf(draggedGroupId);
    const to = ids.indexOf(targetId);
    if (from < 0 || to < 0) return;
    ids.splice(from, 1); ids.splice(to, 0, draggedGroupId);
    setDraggedGroupId(null);
    try { setSettings(await reorderSkillGroups(ids)); } catch (error) { toast.error(error instanceof Error ? error.message : t('skills.saveFailed')); }
  }

  async function dropSkill(groupId: string | null, targetName: string) {
    if (!draggedSkillName || embedded || !visibleSettings) return;
    if (groupId === 'builtin' || visibleSettings.groups.some((group) => group.id === groupId && isBuiltinGroup(group)) || visibleSettings.defaults.some((skill) => skill.name === draggedSkillName && skill.is_builtin)) return;
    const items = visibleSettings.defaults.filter((skill) => skill.group_id === groupId).map((skill) => skill.name).filter((name) => name !== draggedSkillName);
    const targetIndex = items.indexOf(targetName);
    items.splice(Math.max(0, targetIndex), 0, draggedSkillName);
    try {
      let next = await assignSkillGroup(draggedSkillName, groupId);
      next = await reorderSkillGroupItems(groupId ?? 'ungrouped', items);
      setSettings(next);
    } catch (error) { toast.error(error instanceof Error ? error.message : t('skills.saveFailed')); }
    finally { setDraggedSkillName(null); }
  }
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
      setSelectedContentPath(displaySkillFilePath(next.path || 'SKILL.md', next.cwd));
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
          setSelectedContentPath(displaySkillFilePath(nextContent.path || selectedContentPath || 'SKILL.md', nextContent.cwd));
        } else if (activeSkill?.name && !currentManifest) {
          setActive(null);
          setContent(null);
          setSelectedContentPath('SKILL.md');
          onBackToList();
        } else if (active?.kind === 'conflict' && !nextSettings?.conflicts.some((item) => item.name === active.name)) {
          setActive(null);
          onBackToList();
        } else if (active?.kind === 'orphan' && !nextSettings?.orphan_skills.includes(active.name)) {
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

  function selectOrphan(name: string) {
    setActive({ kind: 'orphan', name });
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
        importGroupId || undefined,
      );
      setImportTasks((current) => [task, ...current.filter((item) => item.id !== task.id)]);
      pendingImportTasks.current.add(task.id);
      setImportOpen(false);
      setImportMode('local');
      setImportLocalPath('');
      setImportNetworkUrl('');
      setImportProxyId('');
      setImportGroupId('');
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

  async function toggleAgentSkill(skill: SkillManifest, value: boolean | null) {
    if (!activeAgentId || !agentSettings) return;
    const skills = agentSettings.overrides.skills.filter((item) => item.name !== skill.name);
    skills.push({ name: skill.name, enabled: value });
    const next: SkillOverrides = { skills, groups: [] };
    setBusy(true);
    try { setAgentSettings(await updateAgentSkillsConfig(activeAgentId, next)); } catch (error) { toast.error(error instanceof Error ? error.message : t('skills.saveFailed')); }
    finally { setBusy(false); }
  }

  async function cleanOrphans() {
    const orphanSkills = settings?.orphan_skills ?? [];
    const accepted = await confirm({
      title: t('skills.cleanOrphansTitle'),
      message: t('skills.cleanOrphansConfirm', {
        skills: orphanSkills.length,
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
    setIconOpen(true);
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
                  <div className="skills-add-actions">
                    <button type="button" className="skills-add-group-button" onClick={openCreateGroup} disabled={busy} title={t('skills.newGroup')} aria-label={t('skills.newGroup')}>
                      <FolderPlus size={15} />
                    </button>
                    <IconButton variant="ghost" type="button" label={t('skills.add')} onClick={() => void openImportDialog()} disabled={busy || importing}>
                      <Plus size={18} />
                    </IconButton>
                  </div>
                ) : null}
              </>
            )}
          />
          {selectedSkills.length > 0 && !embedded ? (
            <div className="skills-selection-toolbar">
              <span>{t('skills.selectedCount', { count: selectedSkills.length })}</span>
              <div className="skills-selection-toolbar__actions">
                <div className="skills-move-menu-wrap">
                  <Button variant="secondary" type="button" onClick={() => setMoveMenuOpen((value) => !value)} disabled={busy} icon={<FolderOpen size={15} />}>
                    {t('skills.moveToGroup')}
                  </Button>
                  {moveMenuOpen ? (
                    <div className="skills-group-menu skills-move-menu" role="menu">
                      <button type="button" onClick={() => void moveSkills(null)}>{t('skills.ungrouped')}</button>
                      {(settings?.groups ?? []).filter((group) => !isBuiltinGroup(group)).map((group) => <button key={group.id} type="button" onClick={() => void moveSkills(group.id)}><FolderOpen size={14} />{group.name}</button>)}
                    </div>
                  ) : null}
                </div>
                <IconButton variant="ghost" type="button" label={t('skills.clearSelection')} onClick={() => setSelectedSkills([])}><X size={16} /></IconButton>
              </div>
            </div>
          ) : null}
          <div className="list-stack">
            {groupedSkills.map(({ group, items }) => {
              const key = collapseKey(group);
              const collapsed = collapsedGroups[key] ?? false;
              const builtinGroup = isBuiltinGroup(group);
              const groupName = builtinGroup ? t('skills.builtinGroup') : (group?.name ?? t('skills.ungrouped'));
              return (
              <div key={key} className="skills-group-list" onDragOver={(event) => { if (!embedded && !builtinGroup) event.preventDefault(); }} onDrop={() => { if (!embedded && !builtinGroup && draggedSkillName && !items.some((item) => item.name === draggedSkillName)) void moveSkills(group?.id ?? null, [draggedSkillName]); }}>
                <div className="skills-group-heading" draggable={!embedded && Boolean(group) && !builtinGroup} onDragStart={() => { if (group && !builtinGroup) setDraggedGroupId(group.id); }} onDragEnd={() => setDraggedGroupId(null)} onDragOver={(event) => { if (draggedGroupId && group && !builtinGroup) event.preventDefault(); }} onDrop={() => { if (group && !builtinGroup) void dropGroup(group.id); }}>
                  <button type="button" className="skills-group-toggle" onClick={() => toggleCollapsed(group)} aria-expanded={!collapsed} aria-label={`${collapsed ? t('skills.expandGroup') : t('skills.collapseGroup')}: ${groupName}`}>
                    {collapsed ? <ChevronRight size={15} /> : <ChevronDown size={15} />}
                    <FolderOpen size={15} />
                    <span>{groupName}</span>
                  </button>
                  <span className="skills-group-heading-actions">
                    <span className="skills-group-count">{items.length}</span>
                  {!embedded && group && !builtinGroup ? <div className="skills-group-menu-wrap"><IconButton variant="ghost" type="button" label={t('skills.groupActions')} onClick={(event) => openGroupMenu(event, group.id)}><MoreHorizontal size={16} /></IconButton></div> : null}
                  </span>
                </div>
                {!collapsed ? items.map((skill) => (
                  <div key={skill.name} className={`skills-item-wrap${selectedSkills.includes(skill.name) ? ' is-selected' : ''}`} draggable={!embedded && !skill.is_builtin} onDragStart={() => { if (!skill.is_builtin) setDraggedSkillName(skill.name); }} onDragEnd={() => setDraggedSkillName(null)} onDragOver={(event) => { if (!skill.is_builtin && draggedSkillName && draggedSkillName !== skill.name) event.preventDefault(); }} onDrop={() => void dropSkill(group?.id ?? null, skill.name)}>
                    {!embedded ? <button type="button" className="skills-select-button" disabled={skill.is_builtin} aria-label={selectedSkills.includes(skill.name) ? t('skills.deselectSkill', { name: skill.name }) : t('skills.selectSkill', { name: skill.name })} onClick={() => toggleSkillSelection(skill.name)}>{selectedSkills.includes(skill.name) ? <CheckSquare size={15} /> : <span />}</button> : null}
                    <div className="skills-list-item-row">
                      <ListItem className="skills-list-item" type="button" active={active?.kind === 'skill' && active.name === skill.name} title={<span className="tool-list-title"><span>{skill.name}</span></span>} subtitle={<span className="skills-list-description" title={skill.description}>{skill.description}</span>} leading={<SkillIcon iconUrl={skill.icon_url} size={19} />} onClick={() => selectSkill(skill.name)} />
                      {!embedded ? (
                        <span className="skills-list-item-actions">
                          <IconButton className="skills-move-action" variant="ghost" type="button" label={t('skills.moveSkill')} onClick={() => openMoveSkillDialog(skill)} disabled={busy || skill.is_builtin}>
                            <FolderInput size={16} />
                          </IconButton>
                          <span className={`skills-state-dot ${skill.enabled ? 'is-enabled' : 'is-disabled'}`} title={skill.enabled ? t('skills.enabled') : t('skills.disabled')} aria-label={skill.enabled ? t('skills.enabled') : t('skills.disabled')} />
                        </span>
                      ) : null}
                    </div>
                  </div>
                )) : null}
              </div>
              );
            })}
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
            {settings && settings.orphan_skills.length > 0 ? (
              <div className="skills-orphan-list">
                <div className="skills-group-heading skills-conflict-heading">
                  <span>{t('skills.orphanCategory')}</span>
                  <span className="mini-status-badge skills-conflict-badge"><AlertCircle size={11} />{settings.orphan_skills.length}</span>
                </div>
                {settings.orphan_skills.map((name) => (
                  <ListItem
                    key={`orphan-skill:${name}`}
                    className="skills-list-item skills-orphan-item"
                    type="button"
                    active={active?.kind === 'orphan' && active.name === name}
                    title={<span className="tool-list-title"><span>{name}</span><span className="mini-status-badge skills-conflict-badge"><AlertCircle size={11} />{t('skills.orphanBadge')}</span></span>}
                    subtitle={<span className="skills-list-description">{settings.conflicts.some((item) => item.name === name) ? t('skills.orphanConflict') : t('skills.orphanMissing')}</span>}
                    leading={<AlertCircle size={19} />}
                    onClick={() => selectOrphan(name)}
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
                  {activeSkill ? <IconButton variant="danger" type="button" label={t('skills.delete')} onClick={() => void removeActiveSkill()} disabled={busy || activeSkill.is_builtin}><Trash2 size={17} /></IconButton> : null}
                  {activeSkill ? <SkillToggle skill={activeSkill} agentMode={false} busy={busy} onGlobalToggle={toggleGlobal} onAgentToggle={toggleAgentSkill} /> : null}
                </>
              )} />
              {activeOrphan?.exists ? <SkillOrphanDetail orphan={activeOrphan} onClean={() => void cleanOrphans()} busy={busy} /> : null}
              {activeConflict ? <SkillConflictDetail conflict={activeConflict} /> : null}
              {activeSkill && content ? <SkillEditor skill={activeSkill} content={content} selectedPath={selectedContentPath} onPathChange={setSelectedContentPath} /> : null}
            </>
          )}
        </div>
      )}
      />
      {groupMenu && groupMenuPosition ? createPortal(
        <div className="skills-group-menu skills-group-menu--portal" role="menu" style={{ top: groupMenuPosition.top, left: groupMenuPosition.left }}>
          {(() => {
            const group = settings?.groups.find((item) => item.id === groupMenu);
            return (
              <>
                <button type="button" onClick={() => { if (group) openRenameGroup(group); }}><Pencil size={14} />{t('skills.renameGroup')}</button>
                <button type="button" className="is-danger" onClick={() => { if (group) void removeGroup(group); }}><Trash2 size={14} />{t('skills.deleteGroup')}</button>
              </>
            );
          })()}
        </div>,
        document.body,
      ) : null}
      {groupDialog ? (
        <Dialog
          open
          title={groupDialog.mode === 'create' ? t('skills.newGroup') : t('skills.renameGroup')}
          description={t('skills.groupDialogDescription')}
          icon={<FolderOpen size={21} />}
          closeLabel={t('common.close')}
          onClose={() => { if (!busy) setGroupDialog(null); }}
          size="sm"
          footer={(
            <>
              <Button variant="ghost" type="button" onClick={() => setGroupDialog(null)} disabled={busy}>{t('common.cancel')}</Button>
              <Button variant="primary" type="button" onClick={() => void saveGroup()} disabled={!groupDraft.trim() || busy} icon={<CheckSquare size={15} />}>{t('common.save')}</Button>
            </>
          )}
        >
          <FormField label={t('skills.groupName')} htmlFor="skill-group-name">
            <input id="skill-group-name" type="text" value={groupDraft} onChange={(event) => setGroupDraft(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') void saveGroup(); }} maxLength={80} />
          </FormField>
        </Dialog>
      ) : null}
      {moveSkillDialog ? (
        <Dialog
          open
          title={t('skills.moveSkill')}
          description={t('skills.moveSkillDescription', { name: moveSkillDialog.name })}
          icon={<FolderInput size={21} />}
          closeLabel={t('common.close')}
          onClose={() => { if (!busy) setMoveSkillDialog(null); }}
          size="sm"
          footer={(
            <>
              <Button variant="ghost" type="button" onClick={() => setMoveSkillDialog(null)} disabled={busy}>{t('common.cancel')}</Button>
              <Button variant="primary" type="button" onClick={() => void saveMoveSkill()} disabled={busy} icon={<CheckSquare size={15} />}>{t('common.save')}</Button>
            </>
          )}
        >
          <FormField label={t('skills.addToGroup')} htmlFor="skill-move-group">
            <select id="skill-move-group" value={moveSkillDialog.groupId} onChange={(event) => setMoveSkillDialog((current) => current ? { ...current, groupId: event.target.value } : current)} disabled={busy}>
              <option value="">{t('skills.ungrouped')}</option>
              {(settings?.groups ?? []).filter((group) => !isBuiltinGroup(group)).map((group) => <option key={group.id} value={group.id}>{group.name}</option>)}
            </select>
          </FormField>
        </Dialog>
      ) : null}
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
            <label className="form-field skills-import-group-field">
              <span>{t('skills.addToGroup')}</span>
              <select value={importGroupId} onChange={(event) => setImportGroupId(event.target.value)} disabled={importing}>
                <option value="">{t('skills.ungrouped')}</option>
                {(settings?.groups ?? []).filter((group) => !isBuiltinGroup(group)).map((group) => <option key={group.id} value={group.id}>{group.name}</option>)}
              </select>
            </label>
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
    </>
  );
}



