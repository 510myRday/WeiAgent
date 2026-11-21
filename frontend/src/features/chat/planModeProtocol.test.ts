import { expect, test } from 'vitest';
import { parsePlanModeEvent, parsePlanModeInteraction, parsePlanModeInteractionText, stripPlanModeProtocol } from './planModeProtocol';

test('parses structured multi-question function-call payloads', () => {
  const event = parsePlanModeInteraction({
    status: 'questions',
    questions: [{ id: 'scope', question: '范围？', allow_custom: true, options: [{ id: 'all', title: '全部', description: '覆盖所有模块' }] }],
  });
  expect(event).toEqual({ type: 'questions', questions: [{ id: 'scope', question: '范围？', allow_custom: true, selection_mode: 'single', options: [{ id: 'all', title: '全部', description: '覆盖所有模块' }] }] });
});

test('preserves multiple selection mode', () => {
  expect(parsePlanModeInteraction({
    status: 'questions',
    questions: [{ id: 'sections', question: '选择板块', selection_mode: 'multiple', allow_custom: false, options: [] }],
  })).toMatchObject({ type: 'questions', questions: [{ selection_mode: 'multiple' }] });
});

test('parses completed questions from a partial streaming function-call payload', () => {
  const event = parsePlanModeInteractionText('{"status":"questions","questions":[{"id":"scope","question":"范围？","options":[{"id":"all","title":"全部","description":"覆盖所有模块"}');
  expect(event).toMatchObject({ type: 'questions', questions: [{ question: '范围？', options: [{ title: '全部' }] }] });
});

test('parses wrapped plan questions with clickable options', () => {
  const event = parsePlanModeEvent(
    '<weiagent_plan_question>{"question":"部署到哪里？","options":[{"id":"cloud","label":"云端"}],"allow_custom":true}</weiagent_plan_question>',
  );
  expect(event).toEqual({
    type: 'question',
    question: '部署到哪里？',
    options: [{ id: 'cloud', label: '云端' }],
    allow_custom: true,
  });
});

test('accepts a fenced JSON response and removes protocol markup from display', () => {
  const event = parsePlanModeEvent('```json\n{"question":"范围？","options":["全部"]}\n```');
  expect(event?.type).toBe('question');
  expect(stripPlanModeProtocol('<weiagent_plan_ready>{"summary":"完成"}</weiagent_plan_ready>')).toBe('');
});
