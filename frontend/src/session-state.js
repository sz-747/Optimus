export function createSessionState(sessions = []) {
  return {
    sessions: [...sessions],
    selectedId: sessions[0]?.id ?? null,
  };
}

export function addSession(state, session) {
  return {
    sessions: [...state.sessions, session],
    selectedId: session.id,
  };
}

export function selectSession(state, id) {
  return state.sessions.some((session) => session.id === id)
    ? { ...state, selectedId: id }
    : state;
}

export function removeSession(state, id) {
  const index = state.sessions.findIndex((session) => session.id === id);
  if (index === -1) return state;

  const sessions = state.sessions.filter((session) => session.id !== id);
  if (state.selectedId !== id) return { ...state, sessions };

  return {
    sessions,
    selectedId: sessions[index]?.id ?? sessions[index - 1]?.id ?? null,
  };
}
