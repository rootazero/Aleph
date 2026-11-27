import { batch, cleanup, effect, signal, untracked } from "nsh";
import type { PanelSdk } from "../../shell/sdk";
import { newIdempotencyKey } from "../chatIdempotency";

export type QuestionOption = { label: string; description?: string; preview?: string };
export type Question = {
  id: string;
  header?: string;
  prompt: string;
  options: QuestionOption[];
  multi_select?: boolean;
  secret?: boolean;
};

type Pending = { session_key: string; request_id: string; answered: number; questions: Question[] };
type Answer = { kind: "choices"; indices: number[] } | { kind: "text"; text: string };
type Draft = { answer: Answer; accepted: boolean };
type Action = { kind: "submit"; answers: { question_id: string; answer: Answer }[] } | { kind: "cancel" };

export function useClarification(sdk: PanelSdk, session: () => string | null) {
  const pending = signal<Pending | undefined>(undefined);
  const drafts = signal(new Map<string, Draft>());
  const cursor = signal(0);
  const ready = signal(false);
  const writing = signal(false);
  const message = signal<string | undefined>(undefined);
  let read = 0;
  let disposed = false;

  const clear = () => {
    pending.set(undefined);
    drafts.set(new Map());
    cursor.set(0);
  };

  async function refresh() {
    const key = session();
    const ticket = ++read;
    ready.set(false);
    if (!key || disposed || !sdk.active() || sdk.connection().state !== "connected") return;
    const current = () => !disposed && sdk.active() && session() === key && ticket === read;
    try {
      const result = await sdk.invoke("clarification.pending") as { pending: Pending[] };
      if (!current()) return;
      const next = result.pending.find((entry) => entry.session_key === key && entry.request_id &&
        Number.isInteger(entry.answered) && entry.answered >= 0 && entry.answered < entry.questions.length &&
        !!entry.questions[entry.answered]?.id);
      const prior = pending();
      batch(() => {
        if (!next) clear();
        else {
          const sameRequest = prior?.session_key === key && prior.request_id === next.request_id;
          if (!sameRequest) {
            drafts.set(new Map());
            cursor.set(next.answered);
          } else if (prior.answered !== next.answered) {
            const unanswered = new Set(next.questions.slice(next.answered).map((question) => question.id));
            drafts.set(new Map([...drafts()].filter(([id]) => unanswered.has(id))));
            cursor.set(next.answered);
          }
          pending.set(next);
        }
        ready.set(true);
      });
    } catch {
      if (current()) message.set("Could not refresh questions.");
    }
  }

  effect(() => {
    session();
    untracked(() => {
      batch(() => { clear(); message.set(undefined); });
      void refresh();
    });
  });
  const onQuestion = ({ data }: { data: unknown }) => {
    const key = (data as { session_key?: unknown } | null)?.session_key;
    if (!key || key === session()) void refresh();
  };
  sdk.on("stream.ask_user", onQuestion);
  sdk.on("stream.clarification_ended", onQuestion);
  sdk.on("connection.", ({ topic }) => {
    if (topic === "connection.warning" || topic === "connection.stale") void refresh();
  });
  cleanup(() => { disposed = true; });

  const blocked = () => !ready() || writing();
  const question = () => pending()?.questions[cursor()];
  const answer = () => {
    const id = question()?.id;
    return id ? drafts().get(id)?.answer : undefined;
  };
  const canContinue = () => {
    const value = answer();
    return value?.kind === "text" ? !!value.text.trim() : !!value?.indices.length;
  };
  const setAnswer = (value: Answer) => {
    const id = question()?.id;
    if (id && !blocked()) drafts.set(new Map(drafts()).set(id, { answer: value, accepted: false }));
  };
  const advance = () => {
    const id = question()?.id;
    const value = answer();
    if (!id || !value || !canContinue() || blocked()) return;
    batch(() => {
      drafts.set(new Map(drafts()).set(id, { answer: value, accepted: true }));
      cursor.update((index) => index + 1);
    });
  };
  const remaining = () => {
    const request = pending();
    return request ? request.questions.slice(request.answered) : [];
  };
  const back = () => {
    if (!blocked() && cursor() > (pending()?.answered ?? 0)) cursor.update((index) => index - 1);
  };

  async function write(action: Action) {
    const target = pending();
    if (!target || blocked() || disposed || !sdk.active() || session() !== target.session_key ||
      sdk.connection().state !== "connected") return;
    const stillHere = () => !disposed && sdk.active() && session() === target.session_key;
    batch(() => { writing.set(true); message.set(undefined); });
    try {
      const response = await sdk.invoke("clarification.resolve", {
        session_key: target.session_key,
        expected_request_id: target.request_id,
        expected_question_id: target.questions[target.answered].id,
        idempotency_key: newIdempotencyKey(),
        action,
      }) as { status?: string } | null;
      if (!stillHere()) return;
      if (response?.status === (action.kind === "submit" ? "answered" : "cancelled")) {
        if (pending()?.request_id === target.request_id) clear();
      } else {
        message.set("Questions changed. Check the current questionnaire before trying again.");
      }
    } catch {
      if (stillHere()) message.set("Could not confirm the action. Check the current questionnaire before trying again.");
    } finally {
      if (stillHere()) {
        await refresh();
        if (!disposed && sdk.active()) writing.set(false);
      } else if (!disposed && sdk.active()) writing.set(false);
    }
  }

  const review = () => remaining().map((item) => ({ question: item, draft: drafts().get(item.id) }));
  const canSubmit = () => !!pending() && !question() && review().every(({ draft }) => draft?.accepted);
  const submit = () => {
    if (!canSubmit()) return;
    void write({ kind: "submit", answers: review().flatMap(({ question, draft }) =>
      draft ? [{ question_id: question.id, answer: draft.answer }] : []) });
  };

  return { pending, blocked, cursor, question, answer, setAnswer, canContinue, advance, back,
    review, canSubmit, submit, cancel: () => void write({ kind: "cancel" }), message, ready,
    refresh: () => { message.set(undefined); void refresh(); } };
}

export type Clarification = ReturnType<typeof useClarification>;
