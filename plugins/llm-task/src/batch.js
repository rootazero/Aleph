// batch.js — Core batch execution engine for LLM Task plugin

const { renderTemplate } = require("./templates");

const MAX_RETRIES = 3;
const BASE_DELAY_MS = 1000;

// ---------------------------------------------------------------------------
// LLM API call
// ---------------------------------------------------------------------------

/**
 * Call the Anthropic Messages API.
 *
 * @param {string} prompt - User prompt text
 * @param {string} model - Model identifier
 * @param {number} temperature - Sampling temperature
 * @returns {Promise<string>} Text response from the model
 */
async function callLLM(prompt, model = "claude-sonnet-4-5-20241022", temperature = 0.7) {
  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (!apiKey) {
    throw new Error("ANTHROPIC_API_KEY environment variable is not set");
  }

  let lastError;
  for (let attempt = 0; attempt < MAX_RETRIES; attempt++) {
    try {
      const response = await fetch("https://api.anthropic.com/v1/messages", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "x-api-key": apiKey,
          "anthropic-version": "2023-06-01",
        },
        body: JSON.stringify({
          model,
          max_tokens: 4096,
          temperature,
          messages: [{ role: "user", content: prompt }],
        }),
      });

      if (!response.ok) {
        const body = await response.text();
        const err = new Error(`API error ${response.status}: ${body}`);
        err.status = response.status;
        throw err;
      }

      const data = await response.json();
      const textBlock = data.content?.find((b) => b.type === "text");
      return textBlock ? textBlock.text : "";
    } catch (err) {
      lastError = err;
      // Don't retry on 4xx client errors (except 429 rate limit)
      if (err.status && err.status >= 400 && err.status < 500 && err.status !== 429) {
        throw err;
      }
      if (attempt < MAX_RETRIES - 1) {
        const delay = BASE_DELAY_MS * Math.pow(2, attempt);
        await new Promise((resolve) => setTimeout(resolve, delay));
      }
    }
  }
  throw lastError;
}

// ---------------------------------------------------------------------------
// Promise pool — concurrency limiter
// ---------------------------------------------------------------------------

/**
 * Run async tasks with a concurrency limit.
 *
 * @param {Array<() => Promise<any>>} tasks - Array of async task functions
 * @param {number} concurrency - Max concurrent tasks
 * @returns {Promise<any[]>} Results in original order
 */
async function promisePool(tasks, concurrency) {
  const results = new Array(tasks.length);
  let nextIndex = 0;

  async function worker() {
    while (nextIndex < tasks.length) {
      const index = nextIndex++;
      try {
        results[index] = { status: "fulfilled", value: await tasks[index]() };
      } catch (err) {
        results[index] = { status: "rejected", reason: err.message || String(err) };
      }
    }
  }

  const workers = Array.from(
    { length: Math.min(concurrency, tasks.length) },
    () => worker(),
  );
  await Promise.all(workers);
  return results;
}

// ---------------------------------------------------------------------------
// Batch operations
// ---------------------------------------------------------------------------

/**
 * Send multiple prompts in parallel with concurrency control.
 */
async function runBatch(prompts, options = {}) {
  const model = options.model || "claude-sonnet-4-5-20241022";
  const maxConcurrent = options.max_concurrent || 5;
  const temperature = options.temperature ?? 0.7;

  const tasks = prompts.map(
    (prompt) => () => callLLM(prompt, model, temperature),
  );

  const results = await promisePool(tasks, maxConcurrent);

  return {
    total: prompts.length,
    succeeded: results.filter((r) => r.status === "fulfilled").length,
    failed: results.filter((r) => r.status === "rejected").length,
    results: results.map((r, i) => ({
      index: i,
      prompt: prompts[i],
      status: r.status,
      response: r.status === "fulfilled" ? r.value : null,
      error: r.status === "rejected" ? r.reason : null,
    })),
  };
}

/**
 * Apply a prompt template to each item and process in parallel.
 */
async function runMap(template, items, options = {}) {
  const prompts = items.map((item) =>
    renderTemplate(template, { item: String(item) }),
  );

  const result = await runBatch(prompts, {
    model: options.model,
    max_concurrent: options.max_concurrent || 3,
    temperature: options.temperature,
  });

  // Enrich results with the original item
  result.results = result.results.map((r, i) => ({
    ...r,
    item: items[i],
  }));

  return result;
}

/**
 * Sequential LLM calls where each step's output feeds into the next.
 */
async function runChain(steps, initialInput = "") {
  const chainResults = [];
  let currentInput = initialInput;

  for (let i = 0; i < steps.length; i++) {
    const step = steps[i];
    const prompt = renderTemplate(step.prompt, { input: currentInput });
    const model = step.model || "claude-sonnet-4-5-20241022";

    try {
      const response = await callLLM(prompt, model, 0.7);
      chainResults.push({
        step: i,
        prompt,
        model,
        status: "fulfilled",
        response,
      });
      currentInput = response;
    } catch (err) {
      chainResults.push({
        step: i,
        prompt,
        model,
        status: "rejected",
        error: err.message || String(err),
      });
      // Stop the chain on failure
      break;
    }
  }

  return {
    total_steps: steps.length,
    completed_steps: chainResults.length,
    final_output: chainResults.length > 0
      ? chainResults[chainResults.length - 1].response || null
      : null,
    steps: chainResults,
  };
}

/**
 * Test a prompt template against expected outputs using an LLM judge.
 */
async function runEvaluate(testCases, promptTemplate, options = {}) {
  const model = options.model || "claude-sonnet-4-5-20241022";
  const judgeModel = options.judge_model || "claude-sonnet-4-5-20241022";

  const evaluations = [];

  for (const tc of testCases) {
    const prompt = renderTemplate(promptTemplate, { input: tc.input });

    let response;
    try {
      response = await callLLM(prompt, model, 0.7);
    } catch (err) {
      evaluations.push({
        input: tc.input,
        expected: tc.expected,
        actual: null,
        score: 0,
        status: "error",
        error: err.message || String(err),
      });
      continue;
    }

    // Use a judge LLM to score the response
    const judgePrompt = [
      "You are an evaluation judge. Score how well the actual output matches the expected output.",
      "Respond with ONLY a JSON object: {\"score\": <0.0 to 1.0>, \"reasoning\": \"<brief explanation>\"}",
      "",
      `Input: ${tc.input}`,
      `Expected output: ${tc.expected}`,
      `Actual output: ${response}`,
    ].join("\n");

    let score = 0;
    let reasoning = "";
    try {
      const judgeResponse = await callLLM(judgePrompt, judgeModel, 0.0);
      const parsed = JSON.parse(judgeResponse);
      score = typeof parsed.score === "number" ? parsed.score : 0;
      reasoning = parsed.reasoning || "";
    } catch {
      reasoning = "Failed to parse judge response";
    }

    evaluations.push({
      input: tc.input,
      expected: tc.expected,
      actual: response,
      score,
      reasoning,
      status: "evaluated",
    });
  }

  const scores = evaluations
    .filter((e) => e.status === "evaluated")
    .map((e) => e.score);
  const avgScore = scores.length > 0
    ? scores.reduce((a, b) => a + b, 0) / scores.length
    : 0;

  return {
    total: testCases.length,
    evaluated: scores.length,
    average_score: Math.round(avgScore * 1000) / 1000,
    evaluations,
  };
}

module.exports = { callLLM, runBatch, runMap, runChain, runEvaluate };
