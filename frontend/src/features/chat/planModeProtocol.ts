export type PlanModeOption = { id: string; title: string; description: string };
export type PlanModeQuestion = { id: string; question: string; options: PlanModeOption[]; allow_custom: boolean; selection_mode: 'single' | 'multiple' };
export type PlanModeReady = { type: 'ready'; title: string; summary: string; steps: string[]; assumptions: string[] };
export type PlanModeQuestions = { type: 'questions'; questions: PlanModeQuestion[] };
export type LegacyPlanModeQuestion = { type: 'question'; question: string; options: Array<{id: string; label: string}>; allow_custom: boolean };
export type PlanModeEvent = PlanModeQuestions | PlanModeReady | LegacyPlanModeQuestion;
const text = (value: unknown) => typeof value === 'string' ? value.trim() : '';
export function parsePlanModeInteraction(value: unknown): PlanModeEvent | null {
  if (!value || typeof value !== 'object') return null;
  const r = value as Record<string, unknown>;
  if (r.status === 'questions' && Array.isArray(r.questions)) {
    const questions = r.questions.flatMap((q, qi) => {
      if (!q || typeof q !== 'object') return [];
      const x = q as Record<string, unknown>;
      const question = text(x.question); if (!question) return [];
      const selection_mode: 'single' | 'multiple' = x.selection_mode === 'multiple' ? 'multiple' : 'single';
      const options = Array.isArray(x.options) ? x.options.flatMap((o, oi) => {
        if (!o || typeof o !== 'object') return [];
        const y = o as Record<string, unknown>; const title = text(y.title) || text(y.label);
        return title ? [{ id: text(y.id) || `option-${oi + 1}`, title, description: text(y.description) }] : [];
      }) : [];
      return [{ id: text(x.id) || `question-${qi + 1}`, question, options, allow_custom: x.allow_custom !== false, selection_mode }];
    });
    return questions.length ? { type: 'questions', questions } : null;
  }
  if (r.status === 'ready' && r.plan && typeof r.plan === 'object') {
    const p = r.plan as Record<string, unknown>;
    return { type: 'ready', title: text(p.title) || 'Plan ready', summary: text(p.summary), steps: Array.isArray(p.steps) ? p.steps.map(text).filter(Boolean) : [], assumptions: Array.isArray(p.assumptions) ? p.assumptions.map(text).filter(Boolean) : [] };
  }
  return null;
}

function balancedJsonObjects(input: string): string[] {
  const objects: string[] = [];
  for (let start = 0; start < input.length; start += 1) {
    if (input[start] !== '{') continue;
    let depth = 0;
    let quoted = false;
    let escaped = false;
    for (let index = start; index < input.length; index += 1) {
      const char = input[index];
      if (quoted) {
        if (escaped) escaped = false;
        else if (char === '\\') escaped = true;
        else if (char === '"') quoted = false;
        continue;
      }
      if (char === '"') { quoted = true; continue; }
      if (char === '{') depth += 1;
      if (char === '}') depth -= 1;
      if (depth === 0) {
        objects.push(input.slice(start, index + 1));
        start = index;
        break;
      }
    }
  }
  return objects;
}

/** Parse the completed question objects already available in a streaming tool-call payload. */
export function parsePlanModeInteractionText(content: string): PlanModeEvent | null {
  try {
    const complete = JSON.parse(content);
    return parsePlanModeInteraction(complete);
  } catch {
    // Continue with the completed question objects in the partial JSON stream.
  }
  if (!/"status"\s*:\s*"questions"/.test(content)) return null;
  const questionsStart = content.indexOf('"questions"');
  const arrayStart = content.indexOf('[', questionsStart);
  if (arrayStart < 0) return null;
  const questions = balancedJsonObjects(content.slice(arrayStart + 1))
    .map((value) => {
      try { return JSON.parse(value) as unknown; } catch { return null; }
    })
    .filter((value): value is Record<string, unknown> => Boolean(value && typeof value === 'object' && 'question' in value));
  if (questions.length) return parsePlanModeInteraction({ status: 'questions', questions });

  // If the first question object is still open, render the question text and
  // every option object that has already closed.
  const questionMatch = content.match(/"question"\s*:\s*("(?:\\.|[^"\\])*")/);
  const optionsKey = content.indexOf('"options"');
  const optionsStart = optionsKey >= 0 ? content.indexOf('[', optionsKey) : -1;
  if (!questionMatch || optionsStart < 0) return null;
  let question = '';
  try { question = JSON.parse(questionMatch[1]) as string; } catch { return null; }
  const options = balancedJsonObjects(content.slice(optionsStart + 1))
    .map((value) => {
      try { return JSON.parse(value) as Record<string, unknown>; } catch { return null; }
    })
    .flatMap((option, index) => {
      const title = option && text(option.title);
      return title ? [{ id: text(option.id) || `option-${index + 1}`, title, description: text(option.description) }] : [];
    });
  return options.length ? { type: 'questions', questions: [{ id: 'question-1', question, options, allow_custom: true, selection_mode: 'single' }] } : null;
}
export function parsePlanModeEvent(content: string): PlanModeEvent | null {
  const match = content.match(/<weiagent_plan_question>\s*([\s\S]*?)\s*<\/weiagent_plan_question>/i)
    ?? content.match(/```json\s*([\s\S]*?)```/i);
  if (!match) return null;
  try {
    const r = JSON.parse(match[1]) as Record<string, unknown>;
    if (typeof r.question !== 'string') return null;
    const options = Array.isArray(r.options) ? r.options.flatMap((o, i) => {
      if (typeof o === 'string') return [{ id: `option-${i + 1}`, label: o }];
      if (!o || typeof o !== 'object') return [];
      const x = o as Record<string, unknown>; const label = text(x.label) || text(x.title);
      return label ? [{ id: text(x.id) || `option-${i + 1}`, label }] : [];
    }) : [];
    return { type: 'question', question: r.question, options, allow_custom: r.allow_custom !== false };
  } catch { return null; }
}
export function stripPlanModeProtocol(content: string): string { return content.replace(/<weiagent_plan_(?:question|ready)>[\s\S]*?<\/weiagent_plan_(?:question|ready)>/gi, '').trim(); }
