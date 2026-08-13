import { CheckCircle2, ChevronDown, Circle, CircleDot, ListChecks } from 'lucide-react';

import type { TaskPlan } from '../../toolTypes';

type Translate = (path: string, params?: Record<string, string | number>) => string;

export function TaskPlanPanel({
  plans,
  isLoading,
  isOpen,
  onToggle,
  t,
}: {
  plans: TaskPlan[];
  isLoading: boolean;
  isOpen: boolean;
  onToggle: () => void;
  t: Translate;
}) {
  const currentPlan = plans[0];
  const completed = currentPlan?.plan.filter((item) => item.status === 'completed').length ?? 0;
  const total = currentPlan?.plan.length ?? 0;

  return (
    <section className={`task-plan-panel${isOpen ? ' is-open' : ''}`} aria-label={t('tools.taskPlanning.name')}>
      <button
        type="button"
        className="task-plan-panel__summary"
        aria-expanded={isOpen}
        onClick={onToggle}
      >
        <span className="task-plan-panel__summary-icon" aria-hidden="true"><ListChecks size={16} /></span>
        <span className="task-plan-panel__summary-copy">
          <strong>{t('tools.taskPlanning.name')}</strong>
          {plans.length > 0 ? (
            <span
              className="task-plan-panel__count-badge"
              title={t('chat.taskPlanHistoryCount', { count: plans.length })}
              aria-label={t('chat.taskPlanHistoryCount', { count: plans.length })}
            >
              {plans.length}
            </span>
          ) : null}
        </span>
        {currentPlan ? (
          <span
            className="task-plan-panel__summary-progress"
            aria-label={t('chat.taskPlanCurrentProgress', { completed, total })}
          >
            <span className="task-plan-panel__progress-count">
              {t('chat.taskPlanCurrentProgress', { completed, total })}
            </span>
            <span className="task-plan-panel__progress-track" aria-hidden="true">
              {currentPlan.plan.map((item, index) => (
                <span
                  key={`${currentPlan.planning_id}-progress-${index}`}
                  className={item.status === 'completed' ? 'is-completed' : undefined}
                />
              ))}
            </span>
          </span>
        ) : null}
        <ChevronDown className="task-plan-panel__chevron" size={17} aria-hidden="true" />
      </button>
      {isOpen ? (
        <div className="task-plan-panel__details">
          {isLoading ? <p className="task-plan-panel__empty">{t('chat.taskPlanLoading')}</p> : null}
          {!isLoading && plans.length === 0 ? <p className="task-plan-panel__empty">{t('chat.taskPlanEmpty')}</p> : null}
          {!isLoading && plans.length > 0 ? (
            <div className="task-plan-panel__scroll">
              <div className="task-plan-history">
                {plans.map((plan, index) => (
                  <TaskPlanHistoryCard
                    key={plan.planning_id}
                    plan={plan}
                    planNumber={plans.length - index}
                    t={t}
                  />
                ))}
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

function TaskPlanHistoryCard({
  plan,
  planNumber,
  t,
}: {
  plan: TaskPlan;
  planNumber: number;
  t: Translate;
}) {
  return (
    <article className="task-plan-history__card">
      <div className="task-plan-history__title-row">
        <span className="task-plan-history__number" aria-label={`第 ${planNumber} 个任务`}>{planNumber}</span>
        <h3
          className="task-plan-history__title"
          title={plan.explanation || t('tools.taskPlanning.name')}
        >
          {plan.explanation || t('tools.taskPlanning.name')}
        </h3>
        <time className="task-plan-history__time" dateTime={plan.created_at}>
          {formatTaskPlanStartTime(plan.created_at)}
        </time>
      </div>
      <ol className="task-plan-panel__list">
        {plan.plan.map((item, index) => (
          <li key={`${plan.planning_id}-${index}-${item.step}`} className={`task-plan-item task-plan-item--${item.status}`}>
            <span className="task-plan-item__index">{index + 1}</span>
            <span
              className="task-plan-item__step"
              title={item.step}
              aria-label={t(`chat.taskPlanStatus${item.status === 'in_progress' ? 'InProgress' : item.status === 'completed' ? 'Completed' : 'Pending'}`)}
            >
              {item.step}
            </span>
            <span className="task-plan-item__status-icon" aria-hidden="true">
              {item.status === 'completed' ? <CheckCircle2 size={17} /> : item.status === 'in_progress' ? <CircleDot size={17} /> : <Circle size={17} />}
            </span>
          </li>
        ))}
      </ol>
    </article>
  );
}

function formatTaskPlanStartTime(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const pad = (part: number) => String(part).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}
