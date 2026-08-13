import type { WebSearchKeyCheckResult } from '../../../toolTypes';
import type { KeyCheckProgress } from './types';

/**
 * Consolidated per-source key management state. Previously this lived in seven
 * separate `Record<string, ...>` useState hooks in WebSearchToolSettings.
 */
export type KeysState = {
  keysDirty: Record<string, boolean>;
  selectedKeyIndices: Record<string, number[]>;
  keyStatuses: Record<string, Record<number, WebSearchKeyCheckResult>>;
  checkingKeys: Record<string, boolean>;
  checkProgress: Record<string, KeyCheckProgress>;
  importingKeys: Record<string, boolean>;
  deletingKeys: Record<string, boolean>;
};

export type KeysFlagMap = 'checkingKeys' | 'importingKeys' | 'deletingKeys';

export type KeysAction =
  | { type: 'setKeysDirtyAll'; keysDirty: Record<string, boolean> }
  | { type: 'markDirty'; sourceId: string }
  | { type: 'setSelected'; sourceId: string; indices: number[] }
  | { type: 'setSelectedAll'; selectedKeyIndices: Record<string, number[]> }
  | { type: 'toggleSelected'; sourceId: string; keyIndex: number; selected: boolean }
  | { type: 'setAllKeyStatuses'; keyStatuses: Record<string, Record<number, WebSearchKeyCheckResult>> }
  | { type: 'setSourceStatuses'; sourceId: string; statuses: Record<number, WebSearchKeyCheckResult> }
  | { type: 'mergeKeyStatus'; sourceId: string; result: WebSearchKeyCheckResult }
  | { type: 'setFlag'; map: KeysFlagMap; sourceId: string; value: boolean }
  | { type: 'setCheckProgress'; sourceId: string; progress: KeyCheckProgress }
  | { type: 'patchCheckProgress'; sourceId: string; patch: (progress: KeyCheckProgress) => KeyCheckProgress };

export function initialKeysState(): KeysState {
  return {
    keysDirty: {},
    selectedKeyIndices: {},
    keyStatuses: {},
    checkingKeys: {},
    checkProgress: {},
    importingKeys: {},
    deletingKeys: {},
  };
}

export function keysReducer(state: KeysState, action: KeysAction): KeysState {
  switch (action.type) {
    case 'setKeysDirtyAll':
      return { ...state, keysDirty: action.keysDirty };
    case 'markDirty':
      return { ...state, keysDirty: { ...state.keysDirty, [action.sourceId]: true } };
    case 'setSelected':
      return {
        ...state,
        selectedKeyIndices: { ...state.selectedKeyIndices, [action.sourceId]: action.indices },
      };
    case 'setSelectedAll':
      return { ...state, selectedKeyIndices: action.selectedKeyIndices };
    case 'toggleSelected': {
      const selectedIndices = new Set(state.selectedKeyIndices[action.sourceId] ?? []);
      if (action.selected) selectedIndices.add(action.keyIndex);
      else selectedIndices.delete(action.keyIndex);
      return {
        ...state,
        selectedKeyIndices: {
          ...state.selectedKeyIndices,
          [action.sourceId]: Array.from(selectedIndices).sort((a, b) => a - b),
        },
      };
    }
    case 'setAllKeyStatuses':
      return { ...state, keyStatuses: action.keyStatuses };
    case 'setSourceStatuses':
      return {
        ...state,
        keyStatuses: { ...state.keyStatuses, [action.sourceId]: action.statuses },
      };
    case 'mergeKeyStatus':
      return {
        ...state,
        keyStatuses: {
          ...state.keyStatuses,
          [action.sourceId]: {
            ...(state.keyStatuses[action.sourceId] ?? {}),
            [action.result.key_index]: action.result,
          },
        },
      };
    case 'setFlag':
      return {
        ...state,
        [action.map]: { ...state[action.map], [action.sourceId]: action.value },
      };
    case 'setCheckProgress':
      return {
        ...state,
        checkProgress: { ...state.checkProgress, [action.sourceId]: action.progress },
      };
    case 'patchCheckProgress': {
      const progress = state.checkProgress[action.sourceId];
      if (!progress) return state;
      return {
        ...state,
        checkProgress: { ...state.checkProgress, [action.sourceId]: action.patch(progress) },
      };
    }
    default:
      return state;
  }
}
