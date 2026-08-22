import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import {
  AlertCircle,
  ArrowLeft,
  CalendarClock,
  CheckCircle2,
  Clock3,
  ExternalLink,
  History,
  Play,
  Plus,
  Save,
  Trash2,
  XCircle,
} from 'lucide-react';

import type {
  Agent,
  Conversation,
  ConversationHeartbeat,
  ConversationHeartbeatExecution,
  ModelProvider,
  ScheduledTask,
  ScheduledTaskExecution,
} from '../../types';
import {
  createScheduledTask,
  deleteHeartbeat,
  deleteScheduledTask,
  listConversations,
  listHeartbeats,
  listHeartbeatExecutions,
  listScheduledTaskExecutions,
  listScheduledTasks,
  runHeartbeat as runHeartbeatRequest,
  runScheduledTask,
  setHeartbeat,
  setHeartbeatEnabled,
  setScheduledTaskEnabled,
  updateScheduledTask,
} from '../../api';
import { useI18n } from '../../i18n';
import { PanelShell } from '../../components/layout';
import { Button, EmptyState, FormField, IconButton, ListItem, PaneHeader, useFeedback } from '../../components/ui';
import { agentDisplayName } from '../../components/AgentListItem';

interface SchedulingPanelProps {
  agents: Agent[];
  providers: ModelProvider[];
  onOpenMain: () => void;
  onBackToList: () => void;
  onOpenConversation: (agentId: string, conversationId: string) => void;
}

type Mode = 'cron' | 'heartbeat';

const EMPTY_CRON = { agentId: '', schedule: '0 9 * * *', prompt: '', timezone: '+08:00' };
const EMPTY_HEARTBEAT = { conversationId: '', interval: '10m', prompt: '' };
type CronForm = typeof EMPTY_CRON;
type HeartbeatForm = typeof EMPTY_HEARTBEAT;
type Translate = (path: string, params?: Record<string, string | number>) => string;

export function SchedulingPanel({ agents, providers, onOpenMain, onBackToList, onOpenConversation }: SchedulingPanelProps) {
  const { t, locale } = useI18n();
  const { toast } = useFeedback();
  const [mode, setMode] = useState<Mode>('cron');
  const [tasks, setTasks] = useState<ScheduledTask[]>([]);
  const [heartbeats, setHeartbeats] = useState<ConversationHeartbeat[]>([]);
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [executions, setExecutions] = useState<ScheduledTaskExecution[]>([]);
  const [heartbeatExecutions, setHeartbeatExecutions] = useState<ConversationHeartbeatExecution[]>([]);
  const [selectedId, setSelectedId] = useState('');
  const [creating, setCreating] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [cronForm, setCronForm] = useState(EMPTY_CRON);
  const [heartbeatForm, setHeartbeatForm] = useState(EMPTY_HEARTBEAT);

  const selectedTask = mode === 'cron' ? tasks.find((task) => task.id === selectedId) : undefined;
  const selectedTaskAgent = selectedTask ? agents.find((agent) => agent.id === selectedTask.agent_id) : undefined;
  const selectedTaskProvider = selectedTaskAgent ? providers.find((provider) => provider.id === selectedTaskAgent.model_provider_id) : undefined;
  const selectedHeartbeat = mode === 'heartbeat'
    ? heartbeats.find((heartbeat) => heartbeat.conversation_id === selectedId)
    : undefined;
  const selectedHeartbeatAgent = selectedHeartbeat ? agents.find((agent) => agent.id === selectedHeartbeat.agent_id) : undefined;
  const selectedHeartbeatProvider = selectedHeartbeatAgent ? providers.find((provider) => provider.id === selectedHeartbeatAgent.model_provider_id) : undefined;
  const conversationById = useMemo(() => new Map(conversations.map((item) => [item.id, item])), [conversations]);
  const modeTitle = mode === 'cron' ? t('scheduling.cronTitle') : t('scheduling.heartbeatTitle');
  const modeDetails = mode === 'cron' ? t('scheduling.cronDetails') : t('scheduling.heartbeatDetails');
  const activeItem = mode === 'cron' ? selectedTask : selectedHeartbeat;
  const cronDirty = creating || Boolean(selectedTask && (cronForm.agentId !== selectedTask.agent_id || cronForm.schedule !== selectedTask.schedule_expr || cronForm.prompt !== selectedTask.prompt || cronForm.timezone !== selectedTask.timezone_offset));
  const heartbeatDirty = creating || Boolean(selectedHeartbeat && (heartbeatForm.conversationId !== selectedHeartbeat.conversation_id || heartbeatForm.interval !== formatInterval(selectedHeartbeat.interval_seconds) || heartbeatForm.prompt !== selectedHeartbeat.prompt));
  const isDirty = mode === 'cron' ? cronDirty : heartbeatDirty;
  const headerActions = (creating ? isDirty : activeItem) ? (
    <EditorActions
      saving={saving}
      t={t}
      dirty={isDirty}
      onSave={mode === 'cron' ? () => void saveCron() : () => void saveHeartbeat()}
      onRun={!creating && selectedTask ? () => void runCron(selectedTask) : !creating && selectedHeartbeat ? () => void runHeartbeat(selectedHeartbeat) : undefined}
      onRemove={!creating && selectedId ? () => void removeCurrent() : undefined}
    />
  ) : undefined;

  const refresh = useCallback(async (options?: { silent?: boolean }) => {
    if (!options?.silent) setLoading(true);
    try {
      const [nextTasks, nextHeartbeats, conversationGroups] = await Promise.all([
        listScheduledTasks(),
        listHeartbeats(),
        Promise.all(agents.map((agent) => listConversations(agent.id))),
      ]);
      setTasks(nextTasks);
      setHeartbeats(nextHeartbeats);
      setConversations(conversationGroups.flat());
      if (mode === 'cron' && selectedId && !nextTasks.some((task) => task.id === selectedId)) setSelectedId('');
      if (mode === 'heartbeat' && selectedId && !nextHeartbeats.some((item) => item.conversation_id === selectedId)) setSelectedId('');
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('scheduling.loadFailed'));
    } finally {
      if (!options?.silent) setLoading(false);
    }
  }, [agents, mode, selectedId, t, toast]);

  useEffect(() => { void refresh(); }, [refresh]);

  useEffect(() => {
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') void refresh({ silent: true });
    };
    const intervalId = window.setInterval(refreshWhenVisible, 30_000);
    document.addEventListener('visibilitychange', refreshWhenVisible);
    return () => {
      window.clearInterval(intervalId);
      document.removeEventListener('visibilitychange', refreshWhenVisible);
    };
  }, [refresh]);

  const selectedTaskId = selectedTask?.id;
  const refreshExecutions = useCallback(async (taskId: string) => {
    try {
      setExecutions(await listScheduledTaskExecutions(taskId));
    } catch {
      setExecutions([]);
    }
  }, []);

  useEffect(() => {
    if (!selectedTaskId) {
      setExecutions([]);
      return;
    }
    void refreshExecutions(selectedTaskId);
    const intervalId = window.setInterval(() => void refreshExecutions(selectedTaskId), 1500);
    return () => window.clearInterval(intervalId);
  }, [refreshExecutions, selectedTaskId]);

  const selectedHeartbeatId = selectedHeartbeat?.conversation_id;
  const refreshHeartbeatExecutions = useCallback(async (conversationId: string) => {
    try {
      setHeartbeatExecutions(await listHeartbeatExecutions(conversationId));
    } catch {
      setHeartbeatExecutions([]);
    }
  }, []);

  useEffect(() => {
    if (!selectedHeartbeatId) {
      setHeartbeatExecutions([]);
      return;
    }
    void refreshHeartbeatExecutions(selectedHeartbeatId);
    const intervalId = window.setInterval(() => void refreshHeartbeatExecutions(selectedHeartbeatId), 1500);
    return () => window.clearInterval(intervalId);
  }, [refreshHeartbeatExecutions, selectedHeartbeatId]);

  function selectMode(nextMode: Mode) {
    setMode(nextMode);
    setCreating(false);
    setSelectedId('');
    onOpenMain();
  }

  function selectTask(task: ScheduledTask) {
    setMode('cron');
    setCreating(false);
    setSelectedId(task.id);
    setCronForm({ agentId: task.agent_id, schedule: task.schedule_expr, prompt: task.prompt, timezone: task.timezone_offset });
    onOpenMain();
  }

  function selectHeartbeat(heartbeat: ConversationHeartbeat) {
    setMode('heartbeat');
    setCreating(false);
    setSelectedId(heartbeat.conversation_id);
    setHeartbeatForm({ conversationId: heartbeat.conversation_id, interval: formatInterval(heartbeat.interval_seconds), prompt: heartbeat.prompt });
    onOpenMain();
  }

  function createCron() {
    setMode('cron');
    setSelectedId('');
    setCreating(true);
    setCronForm({ ...EMPTY_CRON, agentId: agents[0]?.id ?? '' });
    onOpenMain();
  }

  function createHeartbeat() {
    setMode('heartbeat');
    setSelectedId('');
    setCreating(true);
    setHeartbeatForm({ ...EMPTY_HEARTBEAT, conversationId: conversations[0]?.id ?? '' });
    onOpenMain();
  }

  async function saveCron() {
    if (!cronForm.agentId || !cronForm.schedule.trim() || !cronForm.prompt.trim()) {
      toast.warning(t('scheduling.invalidForm'));
      return;
    }
    setSaving(true);
    try {
      const saved = selectedTask && !creating
        ? await updateScheduledTask(selectedTask.id, { agent_id: cronForm.agentId, schedule: cronForm.schedule, prompt: cronForm.prompt, timezone_offset: cronForm.timezone })
        : await createScheduledTask({ agent_id: cronForm.agentId, schedule: cronForm.schedule, prompt: cronForm.prompt, timezone_offset: cronForm.timezone });
      setTasks((current) => current.some((item) => item.id === saved.id) ? current.map((item) => item.id === saved.id ? saved : item) : [saved, ...current]);
      setCreating(false);
      setSelectedId(saved.id);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('scheduling.saveFailed'));
    } finally {
      setSaving(false);
    }
  }

  async function saveHeartbeat() {
    if (!heartbeatForm.conversationId || !heartbeatForm.interval.trim() || !heartbeatForm.prompt.trim()) {
      toast.warning(t('scheduling.invalidForm'));
      return;
    }
    const seconds = parseInterval(heartbeatForm.interval);
    if (!seconds) {
      toast.warning(t('scheduling.intervalHint'));
      return;
    }
    setSaving(true);
    try {
      const previousConversationId = selectedHeartbeat?.conversation_id;
      const saved = await setHeartbeat(previousConversationId ?? heartbeatForm.conversationId, {
        conversation_id: heartbeatForm.conversationId,
        interval_seconds: seconds,
        prompt: heartbeatForm.prompt,
      });
      setHeartbeats((current) => {
        const withoutPrevious = previousConversationId && previousConversationId !== saved.conversation_id
          ? current.filter((item) => item.conversation_id !== previousConversationId)
          : current;
        return withoutPrevious.some((item) => item.conversation_id === saved.conversation_id)
          ? withoutPrevious.map((item) => item.conversation_id === saved.conversation_id ? saved : item)
          : [saved, ...withoutPrevious];
      });
      setCreating(false);
      setSelectedId(saved.conversation_id);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t('scheduling.saveFailed'));
    } finally {
      setSaving(false);
    }
  }

  async function toggleCron(task: ScheduledTask) {
    try {
      const updated = await setScheduledTaskEnabled(task.id, !task.enabled);
      setTasks((current) => current.map((item) => item.id === updated.id ? updated : item));
    } catch (error) { toast.error(error instanceof Error ? error.message : t('scheduling.saveFailed')); }
  }

  async function toggleHeartbeat(item: ConversationHeartbeat) {
    try {
      const updated = await setHeartbeatEnabled(item.conversation_id, !item.enabled);
      setHeartbeats((current) => current.map((heartbeat) => heartbeat.conversation_id === updated.conversation_id ? updated : heartbeat));
    } catch (error) { toast.error(error instanceof Error ? error.message : t('scheduling.saveFailed')); }
  }

  async function runCron(task: ScheduledTask) {
    try {
      const updated = await runScheduledTask(task.id);
      setTasks((current) => current.map((item) => item.id === updated.id ? updated : item));
      void refreshExecutions(task.id);
    }
    catch (error) { toast.error(error instanceof Error ? error.message : t('scheduling.saveFailed')); }
  }

  async function runHeartbeat(item: ConversationHeartbeat) {
    try {
      const updated = await runHeartbeatRequest(item.conversation_id);
      setHeartbeats((current) => current.map((heartbeat) => heartbeat.conversation_id === updated.conversation_id ? updated : heartbeat));
      void refreshHeartbeatExecutions(item.conversation_id);
    }
    catch (error) { toast.error(error instanceof Error ? error.message : t('scheduling.saveFailed')); }
  }

  async function removeCurrent() {
    if (!selectedId || !window.confirm(t('scheduling.deleteConfirm'))) return;
    try {
      if (mode === 'cron') {
        await deleteScheduledTask(selectedId);
        setTasks((current) => current.filter((item) => item.id !== selectedId));
      } else {
        await deleteHeartbeat(selectedId);
        setHeartbeats((current) => current.filter((item) => item.conversation_id !== selectedId));
      }
      setSelectedId('');
      setCreating(false);
    } catch (error) { toast.error(error instanceof Error ? error.message : t('scheduling.saveFailed')); }
  }

  return (
    <PanelShell
      className="scheduling-panel"
      sidebar={(
        <section className="context-pane scheduling-list-pane" aria-label={t('scheduling.title')}>
          <PaneHeader
            title={t('scheduling.title')}
            actions={<IconButton variant="ghost" type="button" label={mode === 'cron' ? t('scheduling.newCron') : t('scheduling.newHeartbeat')} onClick={mode === 'cron' ? createCron : createHeartbeat}><Plus size={18} /></IconButton>}
          />
          <div className="scheduling-list-tabs" role="tablist" aria-label={t('scheduling.title')}>
            <button type="button" role="tab" aria-selected={mode === 'cron'} className={mode === 'cron' ? 'is-active' : ''} onClick={() => selectMode('cron')}>
              <CalendarClock size={16} />
              <span className="scheduling-tab-label">{t('scheduling.cron')}</span>
              <span className="scheduling-tab-count">{tasks.length}</span>
            </button>
            <button type="button" role="tab" aria-selected={mode === 'heartbeat'} className={mode === 'heartbeat' ? 'is-active' : ''} onClick={() => selectMode('heartbeat')}>
              <Clock3 size={16} />
              <span className="scheduling-tab-label">{t('scheduling.heartbeat')}</span>
              <span className="scheduling-tab-count">{heartbeats.length}</span>
            </button>
          </div>
          <div className="scheduling-items">
            {mode === 'cron' && tasks.map((task) => (
              <ListItem
                key={task.id}
                active={task.id === selectedId && !creating}
                onClick={() => selectTask(task)}
                title={task.prompt}
                subtitle={`${task.schedule_expr} · ${agentName(agents, task.agent_id, t)}`}
                trailing={<SchedulingListControl enabled={task.enabled} label={task.prompt} nextRunAt={task.next_run_at} onToggle={() => void toggleCron(task)} onRefresh={() => void refresh({ silent: true })} t={t} />}
              />
            ))}
            {mode === 'heartbeat' && heartbeats.map((item) => (
              <ListItem
                key={item.conversation_id}
                active={item.conversation_id === selectedId && !creating}
                onClick={() => selectHeartbeat(item)}
                title={conversationById.get(item.conversation_id)?.title ?? item.conversation_id}
                subtitle={`${formatInterval(item.interval_seconds)} · ${item.prompt}`}
                trailing={<SchedulingListControl enabled={item.enabled} label={conversationById.get(item.conversation_id)?.title ?? item.conversation_id} nextRunAt={item.next_fire_at} onToggle={() => void toggleHeartbeat(item)} onRefresh={() => void refresh({ silent: true })} t={t} />}
              />
            ))}
            {((mode === 'cron' && tasks.length === 0) || (mode === 'heartbeat' && heartbeats.length === 0)) && !loading ? <EmptyState icon={mode === 'cron' ? <CalendarClock size={22} /> : <Clock3 size={22} />} title={mode === 'cron' ? t('scheduling.noCron') : t('scheduling.noHeartbeat')} /> : null}
          </div>
        </section>
      )}
      main={(
        <div className="main-pane config-main scheduling-main">
          <PaneHeader
            className="config-page-header"
            leading={<Button className="main-back-button" variant="ghost" type="button" onClick={onBackToList} icon={<ArrowLeft size={17} />}>{t('common.back')}</Button>}
            eyebrow={t('scheduling.title')}
            title={creating ? (mode === 'cron' ? t('scheduling.newCron') : t('scheduling.newHeartbeat')) : modeTitle}
            description={modeDetails}
            actions={headerActions}
          />
          {!creating && !selectedId ? <EmptyState icon={mode === 'cron' ? <CalendarClock size={28} /> : <Clock3 size={28} />} title={mode === 'cron' ? t('scheduling.selectCron') : t('scheduling.selectHeartbeat')} /> : null}
          {(creating || (mode === 'cron' && selectedTask)) && mode === 'cron' ? <CronEditor form={cronForm} agents={agents} t={t} onChange={setCronForm} /> : null}
          {(creating || (mode === 'heartbeat' && selectedHeartbeat)) && mode === 'heartbeat' ? <HeartbeatEditor form={heartbeatForm} conversations={conversations} t={t} onChange={setHeartbeatForm} /> : null}
          {mode === 'cron' && selectedTask && !creating ? <ExecutionHistory executions={executions} locale={locale} t={t} task={selectedTask} agent={selectedTaskAgent} provider={selectedTaskProvider} onOpenConversation={onOpenConversation} /> : null}
          {mode === 'heartbeat' && selectedHeartbeat && !creating ? <HeartbeatExecutionHistory executions={heartbeatExecutions} locale={locale} t={t} heartbeat={selectedHeartbeat} agent={selectedHeartbeatAgent} provider={selectedHeartbeatProvider} onOpenConversation={onOpenConversation} /> : null}
        </div>
      )}
    />
  );
}

function SchedulingToggle({ enabled, label, onToggle, t }: { enabled: boolean; label: string; onToggle: () => void; t: Translate }) {
  function handleKeyDown(event: KeyboardEvent<HTMLSpanElement>) {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    event.preventDefault();
    event.stopPropagation();
    onToggle();
  }

  return (
    <span
      className="toggle-switch scheduling-toggle-switch"
      role="switch"
      aria-checked={enabled}
      aria-label={`${label} ${enabled ? t('scheduling.enabled') : t('scheduling.paused')}`}
      tabIndex={0}
      onClick={(event) => { event.stopPropagation(); onToggle(); }}
      onKeyDown={handleKeyDown}
    >
      <span className="toggle-slider" />
    </span>
  );
}

function SchedulingListControl({
  enabled,
  label,
  nextRunAt,
  onToggle,
  onRefresh,
  t,
}: {
  enabled: boolean;
  label: string;
  nextRunAt: string | null | undefined;
  onToggle: () => void;
  onRefresh: () => void;
  t: Translate;
}) {
  const [remainingMs, setRemainingMs] = useState(() => remainingUntil(nextRunAt));
  const expirationNotifiedRef = useRef(false);

  useEffect(() => {
    expirationNotifiedRef.current = false;
    setRemainingMs(remainingUntil(nextRunAt));
    if (!enabled || !nextRunAt) return;
    const intervalId = window.setInterval(() => {
      const remaining = remainingUntil(nextRunAt);
      setRemainingMs(remaining);
      if (remaining != null && remaining <= 0 && !expirationNotifiedRef.current) {
        expirationNotifiedRef.current = true;
        onRefresh();
      }
    }, 1000);
    return () => window.clearInterval(intervalId);
  }, [enabled, nextRunAt, onRefresh]);

  return <div className="scheduling-list-control">
    <SchedulingToggle enabled={enabled} label={label} onToggle={onToggle} t={t} />
    <span className={`scheduling-list-status ${enabled ? 'is-running' : 'is-paused'}`} title={enabled ? formatCountdown(remainingMs) : t('scheduling.paused')}>
      {enabled ? formatCountdown(remainingMs) : t('scheduling.paused')}
    </span>
  </div>;
}

function CronEditor({
  form,
  agents,
  t,
  onChange,
}: {
  form: CronForm;
  agents: Agent[];
  t: Translate;
  onChange: (form: CronForm) => void;
}) {
  return <div className="scheduling-editor entity-form">
    <section className="settings-card">
      <div className="card-header">
        <div className="settings-section-title">
          <h3>{t('scheduling.configuration')}</h3>
          <p>{t('scheduling.configurationHint')}</p>
        </div>
      </div>
      <div className="card-body scheduling-card-body">
        <div className="form-grid two-column scheduling-fields-grid">
          <FormField label={t('scheduling.agent')} htmlFor="scheduling-agent">
            <select id="scheduling-agent" value={form.agentId} onChange={(event) => onChange({ ...form, agentId: event.target.value })}>
              <option value="">{t('scheduling.agent')}</option>
              {agents.map((agent: Agent) => <option key={agent.id} value={agent.id}>{agentDisplayName(agent, t)}</option>)}
            </select>
          </FormField>
          <FormField label={t('scheduling.schedule')} htmlFor="scheduling-schedule" hint={t('scheduling.scheduleHint')}>
            <input id="scheduling-schedule" value={form.schedule} onChange={(event) => onChange({ ...form, schedule: event.target.value })} placeholder="0 9 * * *" />
          </FormField>
          <FormField label={t('scheduling.timezone')} htmlFor="scheduling-timezone">
            <input id="scheduling-timezone" value={form.timezone} onChange={(event) => onChange({ ...form, timezone: event.target.value })} placeholder="+08:00" />
          </FormField>
        </div>
        <FormField label={t('scheduling.prompt')} htmlFor="scheduling-prompt" hint={t('scheduling.promptHint')}>
          <textarea id="scheduling-prompt" value={form.prompt} onChange={(event) => onChange({ ...form, prompt: event.target.value })} rows={7} />
        </FormField>
      </div>
    </section>
  </div>;
}

function HeartbeatEditor({
  form,
  conversations,
  t,
  onChange,
}: {
  form: HeartbeatForm;
  conversations: Conversation[];
  t: Translate;
  onChange: (form: HeartbeatForm) => void;
}) {
  return <div className="scheduling-editor entity-form">
    <section className="settings-card">
      <div className="card-header">
        <div className="settings-section-title">
          <h3>{t('scheduling.configuration')}</h3>
          <p>{t('scheduling.heartbeatConfigurationHint')}</p>
        </div>
      </div>
      <div className="card-body scheduling-card-body">
        <div className="form-grid two-column scheduling-fields-grid">
          <FormField label={t('scheduling.conversation')} htmlFor="scheduling-conversation" hint={conversations.length === 0 ? t('scheduling.noConversations') : t('scheduling.conversationHint')}>
            <select id="scheduling-conversation" value={form.conversationId} onChange={(event) => onChange({ ...form, conversationId: event.target.value })}>
              <option value="">{t('scheduling.conversation')}</option>
              {conversations.map((conversation: Conversation) => <option key={conversation.id} value={conversation.id}>{conversation.title}</option>)}
            </select>
          </FormField>
          <FormField label={t('scheduling.interval')} htmlFor="scheduling-interval" hint={t('scheduling.intervalHint')}>
            <input id="scheduling-interval" value={form.interval} onChange={(event) => onChange({ ...form, interval: event.target.value })} placeholder="10m" />
          </FormField>
        </div>
        <FormField label={t('scheduling.prompt')} htmlFor="scheduling-heartbeat-prompt" hint={t('scheduling.promptHint')}>
          <textarea id="scheduling-heartbeat-prompt" value={form.prompt} onChange={(event) => onChange({ ...form, prompt: event.target.value })} rows={7} />
        </FormField>
      </div>
    </section>
  </div>;
}

function EditorActions({ saving, t, dirty, onSave, onRun, onRemove }: {
  saving: boolean;
  t: Translate;
  dirty: boolean;
  onSave: () => void;
  onRun?: () => void;
  onRemove?: () => void;
}) {
  return <div className="scheduling-header-actions">
    {onRun ? <IconButton className="scheduling-run-action" variant="ghost" type="button" label={t('scheduling.runNow')} onClick={onRun}><Play size={17} /></IconButton> : null}
    {onRemove ? <IconButton variant="danger" type="button" label={t('scheduling.remove')} onClick={onRemove}><Trash2 size={17} /></IconButton> : null}
    {dirty ? <IconButton className="scheduling-save-action" variant="default" type="button" disabled={saving} label={saving ? t('common.saving') : t('scheduling.saveChanges')} onClick={onSave}><Save size={17} /></IconButton> : null}
  </div>;
}

function ExecutionHistory({
  executions,
  locale,
  t,
  task,
  agent,
  provider,
  onOpenConversation,
}: {
  executions: ScheduledTaskExecution[];
  locale: string;
  t: Translate;
  task: ScheduledTask;
  agent?: Agent;
  provider?: ModelProvider;
  onOpenConversation: (agentId: string, conversationId: string) => void;
}) {
  return <section className="settings-card scheduling-history">
    <div className="card-header">
      <div className="settings-section-title"><h3><History size={16} />{t('scheduling.executions')}</h3></div>
    </div>
    <div className="card-body scheduling-history-body">
      {executions.length === 0 ? <p className="scheduling-muted">{t('scheduling.noExecutions')}</p> : <div className="scheduling-history-list">{executions.map((execution) => {
        const status = executionStatus(execution.status, t);
        return <article key={execution.id} className={`scheduling-history-row is-${status.kind}`}>
          <div className="scheduling-history-row-header">
            <div className="scheduling-history-meta">
              <span>{t('scheduling.agent')}: {execution.agent_name ?? (agent ? agentDisplayName(agent, t) : task.agent_id)}</span>
              <span>{t('scheduling.model')}: {execution.provider_name ?? provider?.name ?? agent?.model_provider_id ?? '—'} / {execution.model_id ?? agent?.model_id ?? '—'}</span>
              {execution.started_at ? <span>{t('scheduling.startedAt')}: {formatDate(execution.started_at, locale)}</span> : null}
              {execution.finished_at ? <span>{t('scheduling.finishedAt')}: {formatDate(execution.finished_at, locale)}</span> : null}
              {execution.started_at && execution.finished_at ? <span><Clock3 size={14} />{t('scheduling.duration')}: {formatDuration(execution.started_at, execution.finished_at, locale)}</span> : null}
            </div>
            <div className="scheduling-history-status">
              {execution.trigger_type === 'manual' ? <span className="scheduling-trigger-badge">{t('scheduling.manualExecution')}</span> : null}
              <span className={`scheduling-status-badge is-${status.kind}`}><status.icon size={14} />{status.label}</span>
            </div>
          </div>
          <div className="scheduling-history-result">
            {execution.output_preview ? <p className="scheduling-output-preview" title={execution.output_preview}>{execution.output_preview}</p> : null}
            <Button
              className="scheduling-open-conversation"
              variant="ghost"
              type="button"
              icon={<ExternalLink size={14} />}
              onClick={() => onOpenConversation(task.agent_id, execution.conversation_id)}
            >{t('scheduling.openConversation')}</Button>
          </div>
          {execution.error ? <p className="scheduling-error"><AlertCircle size={14} />{execution.error}</p> : null}
        </article>;
      })}</div>}
    </div>
  </section>;
}

function HeartbeatExecutionHistory({
  executions,
  locale,
  t,
  heartbeat,
  agent,
  provider,
  onOpenConversation,
}: {
  executions: ConversationHeartbeatExecution[];
  locale: string;
  t: Translate;
  heartbeat: ConversationHeartbeat;
  agent?: Agent;
  provider?: ModelProvider;
  onOpenConversation: (agentId: string, conversationId: string) => void;
}) {
  return <section className="settings-card scheduling-history">
    <div className="card-header">
      <div className="settings-section-title"><h3><History size={16} />{t('scheduling.executions')}</h3></div>
    </div>
    <div className="card-body scheduling-history-body">
      {executions.length === 0 ? <p className="scheduling-muted">{t('scheduling.noExecutions')}</p> : <div className="scheduling-history-list">{executions.map((execution) => {
        const status = executionStatus(execution.status, t);
        return <article key={execution.id} className={`scheduling-history-row is-${status.kind}`}>
          <div className="scheduling-history-row-header">
            <div className="scheduling-history-meta">
              <span>{t('scheduling.agent')}: {execution.agent_name ?? (agent ? agentDisplayName(agent, t) : heartbeat.agent_id)}</span>
              <span>{t('scheduling.model')}: {execution.provider_name ?? provider?.name ?? agent?.model_provider_id ?? '—'} / {execution.model_id ?? agent?.model_id ?? '—'}</span>
              {execution.started_at ? <span>{t('scheduling.startedAt')}: {formatDate(execution.started_at, locale)}</span> : null}
              {execution.finished_at ? <span>{t('scheduling.finishedAt')}: {formatDate(execution.finished_at, locale)}</span> : null}
              {execution.started_at && execution.finished_at ? <span><Clock3 size={14} />{t('scheduling.duration')}: {formatDuration(execution.started_at, execution.finished_at, locale)}</span> : null}
            </div>
            <div className="scheduling-history-status">
              <span className={`scheduling-status-badge is-${status.kind}`}><status.icon size={14} />{status.label}</span>
            </div>
          </div>
          <div className="scheduling-history-result">
            {execution.output_preview ? <p className="scheduling-output-preview" title={execution.output_preview}>{execution.output_preview}</p> : null}
            <Button
              className="scheduling-open-conversation"
              variant="ghost"
              type="button"
              icon={<ExternalLink size={14} />}
              onClick={() => onOpenConversation(heartbeat.agent_id, heartbeat.conversation_id)}
            >{t('scheduling.openConversation')}</Button>
          </div>
          {execution.error ? <p className="scheduling-error"><AlertCircle size={14} />{execution.error}</p> : null}
        </article>;
      })}</div>}
    </div>
  </section>;
}

function executionStatus(status: string, t: Translate): { kind: 'completed' | 'failed' | 'cancelled' | 'running' | 'pending'; label: string; icon: typeof CheckCircle2 } {
  if (status === 'completed') return { kind: 'completed', label: t('scheduling.statusCompleted'), icon: CheckCircle2 };
  if (status === 'failed') return { kind: 'failed', label: t('scheduling.statusFailed'), icon: XCircle };
  if (status === 'cancelled') return { kind: 'cancelled', label: t('scheduling.statusCancelled'), icon: XCircle };
  if (status === 'running') return { kind: 'running', label: t('scheduling.statusRunning'), icon: Clock3 };
  return { kind: 'pending', label: t('scheduling.statusPending'), icon: Clock3 };
}

function agentName(agents: Agent[], id: string, t: Translate) {
  const agent = agents.find((item) => item.id === id);
  return agent ? agentDisplayName(agent, t) : id;
}
function formatDate(value: string, locale: string) { return new Date(value).toLocaleString(locale === 'zh' ? 'zh-CN' : 'en-US'); }
function formatDuration(start: string, finish: string, locale: string) {
  const milliseconds = Math.max(0, new Date(finish).getTime() - new Date(start).getTime());
  if (milliseconds < 1000) return locale === 'zh' ? `${milliseconds} 毫秒` : `${milliseconds} ms`;
  const seconds = milliseconds / 1000;
  if (seconds < 60) return locale === 'zh' ? `${seconds.toFixed(1)} 秒` : `${seconds.toFixed(1)} s`;
  const minutes = Math.floor(seconds / 60);
  const remainder = Math.round(seconds % 60);
  return locale === 'zh' ? `${minutes} 分 ${remainder} 秒` : `${minutes}m ${remainder}s`;
}
function parseInterval(value: string) { const match = value.trim().match(/^(\d+)\s*([smhd]?)$/i); if (!match) return 0; const unit = match[2].toLowerCase(); const multiplier = unit === 'd' ? 86400 : unit === 'h' ? 3600 : unit === 'm' ? 60 : 1; return Number(match[1]) * multiplier; }
function formatInterval(seconds: number) { if (seconds % 86400 === 0) return `${seconds / 86400}d`; if (seconds % 3600 === 0) return `${seconds / 3600}h`; if (seconds % 60 === 0) return `${seconds / 60}m`; return `${seconds}s`; }
function remainingUntil(value: string | null | undefined) {
  if (!value) return null;
  const timestamp = new Date(value).getTime();
  return Number.isFinite(timestamp) ? Math.max(0, timestamp - Date.now()) : null;
}
function formatCountdown(milliseconds: number | null) {
  if (milliseconds == null) return '—';
  const totalSeconds = Math.floor(milliseconds / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  return `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`;
}
