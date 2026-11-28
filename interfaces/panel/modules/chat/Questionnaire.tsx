import { Scroll, signal, Text, View } from "nsh";
import { Button, Checkbox, Input, Radio, RadioGroup, TextView } from "nsh/base";
import type { Clarification, Question, QuestionOption } from "./clarification";

const secondary = "rounded border border-input px-3 py-2 hover:bg-muted focus-visible:bg-muted disabled:opacity-40";
const primary = "rounded bg-primary px-3 py-2 focus-visible:bg-primary/80 disabled:opacity-40";
const optionClass = "flex-1 flex-row items-center gap-2 rounded px-2 py-2 hover:bg-muted focus-visible:bg-muted";

function Choice(props: {
  option: QuestionOption; multiple: boolean; checked: boolean; blocked: boolean; choose: (checked: boolean) => void;
}) {
  const open = signal(false);
  const label = () => <View class="flex-1 gap-0.5">
    <Text class="text-foreground">{props.option.label}</Text>
    <If when={props.option.description}><Text class="text-muted-foreground">{props.option.description}</Text></If>
  </View>;
  return <View class="gap-1">
    <View class="flex-row items-start gap-2 rounded border border-input">
      <If when={props.multiple} else={
        <Radio checked={props.checked} disabled={props.blocked} accessibilityLabel={props.option.label}
          onChange={({ checked }) => props.choose(checked)} class={`group ${optionClass}`}>
          <View class="size-4 shrink-0 items-center justify-center rounded-full border border-input group-checked:border-primary">
            <View class="hidden size-2 rounded-full bg-primary group-checked:flex" />
          </View>
          {label()}
        </Radio>
      }>
        <Checkbox checked={props.checked} disabled={props.blocked} accessibilityLabel={props.option.label}
          onChange={({ checked }) => props.choose(checked)} class={optionClass}
          indicatorClass="size-4 shrink-0 items-center justify-center rounded border border-input checked:border-primary checked:bg-primary"
          indicator={<If when={props.checked}><Text class="text-primary-foreground">✓</Text></If>}>
          {label()}
        </Checkbox>
      </If>
      <If when={props.option.preview}>
        <Button accessibilityLabel={`${open() ? "Hide preview for" : "Preview"} ${props.option.label}`}
          onPress={() => open.update((value) => !value)} class="my-1 mr-1 shrink-0 rounded px-2 py-1 hover:bg-muted focus-visible:bg-muted">
          <Text class="text-foreground">{open() ? "Hide preview" : "Preview"}</Text>
        </Button>
      </If>
    </View>
    <If when={open() && props.option.preview}>
      <View class="gap-2 rounded border border-input bg-muted/40 p-3">
        <TextView markdown={props.option.preview ?? ""} options={{ html: false, mdx: false }} links="report" selectable
          renderImage={({ alt }) => <Text class="text-muted-foreground">{alt || "Image"}</Text>} />
      </View>
    </If>
  </View>;
}

function QuestionEditor(props: { state: Clarification; question: Question }) {
  const { state } = props;
  const current = state.answer;
  const textMode = () => !props.question.options.length || current()?.kind === "text";
  const text = () => {
    const answer = current();
    return answer?.kind === "text" ? answer.text : "";
  };
  const indices = () => {
    const answer = current();
    return answer?.kind === "choices" ? answer.indices : [];
  };
  const choose = (index: number, checked: boolean) => {
    if (!props.question.multi_select && !checked) return;
    const previous = indices().filter((value) => value !== index);
    const next = props.question.multi_select ? (checked ? [...previous, index] : previous) : [index];
    state.setAnswer({ kind: "choices", indices: next });
  };
  const options = () => <Each of={props.question.options} key="index">
    {(option, index) => <Choice option={option()} multiple={!!props.question.multi_select} blocked={state.blocked()}
      checked={indices().includes(index())} choose={(checked) => choose(index(), checked)} />}
  </Each>;
  return <View class="gap-2">
    <If when={props.question.header}><Text class="text-sm text-muted-foreground">{props.question.header}</Text></If>
    <Text class="font-semibold text-foreground">{props.question.prompt}</Text>
    <If when={!textMode()}>
      <If when={props.question.multi_select} else={
        <RadioGroup accessibilityLabel={props.question.prompt} class="gap-2">{options()}</RadioGroup>
      }><View class="gap-2">{options()}</View></If>
    </If>
    <If when={textMode()}>
      <Input value={text()}
        accessibilityLabel="Your answer" placeholder="Your answer" masked={!!props.question.secret}
        onChange={({ value }) => state.setAnswer({ kind: "text", text: value })}
        class="rounded border border-input px-3 py-2 text-foreground" />
    </If>
    <If when={props.question.options.length > 0}>
      <Button class={`self-start ${secondary}`} disabled={state.blocked()}
        onPress={() => state.setAnswer(textMode() ? { kind: "choices", indices: [] } : { kind: "text", text: "" })}>
        <Text>{textMode() ? "Use choices" : "Enter your own answer"}</Text>
      </Button>
    </If>
  </View>;
}

export function Questionnaire(props: { state: Clarification }) {
  const { state } = props;
  const choiceSummary = (question: Question, indices: number[]) => indices.map((index) => question.options[index]?.label).join(", ");
  const displayed = () => {
    const request = state.pending();
    const question = state.question();
    return request && question ? [{ key: `${request.request_id}:${state.cursor()}`, question }] : [];
  };
  return <If when={state.pending() || state.message()}>
    <View class="shrink-0 gap-2 rounded-(--radius) border border-input bg-background p-3">
      <If when={state.message()}>
        <Text class="text-destructive">{state.message()}</Text>
        <If when={!state.ready()}>
          <Button class={`self-start ${secondary}`} onPress={state.refresh}><Text>Refresh questions</Text></Button>
        </If>
      </If>
      <If when={state.pending()}>
        <Text class="text-sm text-muted-foreground">{state.question() ?
          `Question ${state.cursor() + 1} of ${state.pending()?.questions.length}` : "Review your answers"}</Text>
        <Scroll class="max-h-64" contentClass="gap-2">
          <Each of={displayed()} key={(entry) => entry.key}>
            {(entry) => <QuestionEditor state={state} question={entry().question} />}
          </Each>
          <If when={!state.question()}>
            <Each of={state.review()} key={(entry) => entry.question.id}>
              {(entry) => {
                const summary = () => {
                  if (entry().question.secret) return "Hidden answer";
                  const answer = entry().draft?.answer;
                  return answer?.kind === "choices" ? choiceSummary(entry().question, answer.indices) : answer?.text;
                };
                return <View class="gap-0.5 rounded border border-input p-2">
                  <Text class="font-semibold text-foreground">{entry().question.prompt}</Text>
                  <Text class="text-foreground">{summary()}</Text>
                </View>;
              }}
            </Each>
          </If>
        </Scroll>
        <View class="flex-row flex-wrap items-center gap-2">
          <Button disabled={state.blocked()} onPress={state.cancel} class={secondary}><Text>Cancel questions</Text></Button>
          <If when={state.cursor() > (state.pending()?.answered ?? 0)}>
            <Button disabled={state.blocked()} onPress={state.back} class={secondary}><Text>Back</Text></Button>
          </If>
          <If when={state.question()} else={
            <Button disabled={state.blocked() || !state.canSubmit()} onPress={state.submit} class={primary}>
              <Text class="text-primary-foreground">Submit answers</Text>
            </Button>
          }>
            <Button disabled={state.blocked() || !state.canContinue()} onPress={state.advance} class={primary}>
              <Text class="text-primary-foreground">{state.cursor() + 1 === state.pending()?.questions.length ? "Review answers" : "Continue"}</Text>
            </Button>
          </If>
        </View>
      </If>
    </View>
  </If>;
}
