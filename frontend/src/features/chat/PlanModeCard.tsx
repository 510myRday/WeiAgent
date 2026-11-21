import { Check, ChevronLeft, ChevronRight, CircleHelp, ListChecks, LoaderCircle } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useI18n } from '../../i18n';
import type { PlanModeEvent } from './planModeProtocol';

type Props = { event: PlanModeEvent; disabled: boolean; onAnswer: (answer: string) => void; onExecute: () => void; onRevise: () => void };
type AnswerValue = string | string[];

function hasAnswer(value: AnswerValue | undefined): boolean {
  return Array.isArray(value) ? value.length > 0 : Boolean(value?.trim());
}

export function PlanModeLoadingCard() {
  const { t } = useI18n();
  return (
    <section className="plan-mode-card plan-mode-card--loading" role="status" aria-live="polite">
      <div className="plan-mode-card__heading">
        <LoaderCircle size={18} className="is-spinning" aria-hidden="true" />
        <strong>{t('chat.planModeGeneratingQuestions')}</strong>
      </div>
      <div className="plan-mode-loading__line" />
      <div className="plan-mode-loading__line plan-mode-loading__line--short" />
      <span className="plan-mode-loading__hint">{t('chat.planModeGeneratingQuestions')}</span>
    </section>
  );
}

export function PlanModeCard({ event, disabled, onAnswer, onExecute, onRevise }: Props) {
  const { t } = useI18n();
  const [index, setIndex] = useState(0);
  const [answers, setAnswers] = useState<Record<string, AnswerValue>>({});
  const [custom, setCustom] = useState<Record<string, boolean>>({});
  const eventKey = JSON.stringify(event);
  useEffect(() => { setIndex(0); setAnswers({}); setCustom({}); }, [eventKey]);

  if (event.type === 'question') return <section className="plan-mode-card"><p className="plan-mode-card__question">{event.question}</p>{event.options.map((o, i) => <button type="button" key={o.id} disabled={disabled} onClick={() => onAnswer(o.label)}>{i + 1}. {o.label}</button>)}</section>;
  if (event.type === 'questions') {
    const question = event.questions[index];
    const multiple = question.selection_mode === 'multiple';
    const value = answers[question.id] ?? (multiple ? [] : '');
    const isComplete = hasAnswer(value);
    const last = index === event.questions.length - 1;
    const chooseOption = (optionId: string) => {
      setCustom(current => ({ ...current, [question.id]: false }));
      setAnswers(current => {
        if (multiple) {
          const selected = Array.isArray(current[question.id]) ? current[question.id] as string[] : [];
          return { ...current, [question.id]: selected.includes(optionId) ? selected.filter(id => id !== optionId) : [...selected, optionId] };
        }
        return { ...current, [question.id]: optionId };
      });
      if (!multiple && !last) setIndex(current => current + 1);
    };
    return <section className="plan-mode-card" aria-label={t('chat.planModeQuestion')}>
      <div className="plan-mode-card__heading"><CircleHelp size={18} /><strong>{t('chat.planModeQuestion')} {index + 1}/{event.questions.length}</strong></div>
      <div className="plan-mode-question"><p className="plan-mode-card__question">{question.question}</p><div className="plan-mode-card__options">
        {question.options.map((option, optionIndex) => { const selected = Array.isArray(value) ? value.includes(option.id) : value === option.id; return <button key={option.id} type="button" disabled={disabled} className={!custom[question.id] && selected ? 'is-selected' : ''} onClick={() => chooseOption(option.id)}><span className="plan-mode-option-number">{optionIndex + 1}</span><span><strong>{option.title}</strong><small>{option.description}</small></span></button>; })}
        {question.allow_custom ? <><button type="button" disabled={disabled} className={custom[question.id] ? 'is-selected' : ''} onClick={() => { setCustom(current => ({ ...current, [question.id]: true })); setAnswers(current => ({ ...current, [question.id]: '' })); }}><span className="plan-mode-option-number">{question.options.length + 1}</span><span><strong>{t('chat.planModeCustomAnswer')}</strong></span></button>{custom[question.id] ? <input value={typeof value === 'string' ? value : ''} onChange={e => setAnswers(current => ({ ...current, [question.id]: e.target.value }))} onKeyDown={e => { if (e.key === 'Enter' && hasAnswer(value) && !e.nativeEvent.isComposing && !last) { e.preventDefault(); setIndex(current => current + 1); } }} placeholder={t('chat.planModeCustomAnswer')} /> : null}</> : null}
      </div></div>
      <div className="plan-mode-card__actions"><button type="button" disabled={disabled || index === 0} onClick={() => setIndex(current => current - 1)}><ChevronLeft size={15} />{t('chat.planModePrevious')}</button>{last ? <button type="button" className="is-primary" disabled={disabled || !isComplete || event.questions.some(q => !hasAnswer(answers[q.id]))} onClick={() => onAnswer(JSON.stringify(answers))}>{t('chat.planModeContinue')}</button> : <button type="button" className="is-primary" disabled={disabled || !isComplete} onClick={() => setIndex(current => current + 1)}>{t('chat.planModeNext')}<ChevronRight size={15} /></button>}</div>
    </section>;
  }
  return <section className="plan-mode-card plan-mode-card--ready" aria-label={t('chat.planModeReady')}><div className="plan-mode-card__heading"><ListChecks size={18} /><strong>{event.title}</strong></div><p className="plan-mode-card__question">{event.summary}</p><ol className="plan-mode-card__steps">{event.steps.map((step, i) => <li key={`${i}-${step}`}>{step}</li>)}</ol><div className="plan-mode-card__actions"><button type="button" disabled={disabled} onClick={onRevise}>{t('chat.planModeRevise')}</button><button type="button" className="is-primary" disabled={disabled} onClick={onExecute}><Check size={15} />{t('chat.planModeExecute')}</button></div></section>;
}
