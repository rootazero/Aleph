// templates.js — Simple template engine for LLM Task plugin

/**
 * Replace all {{key}} placeholders in a template with values from vars.
 *
 * @param {string} template - Template string with {{key}} placeholders
 * @param {Record<string, string>} vars - Key-value pairs for substitution
 * @returns {string} Rendered string
 */
function renderTemplate(template, vars) {
  return template.replace(/\{\{(\w+)\}\}/g, (match, key) => {
    return key in vars ? String(vars[key]) : match;
  });
}

/**
 * Extract all unique {{key}} placeholder names from a template.
 *
 * @param {string} template - Template string
 * @returns {string[]} Array of variable names (deduplicated)
 */
function extractVariables(template) {
  const matches = template.matchAll(/\{\{(\w+)\}\}/g);
  const seen = new Set();
  for (const m of matches) {
    seen.add(m[1]);
  }
  return [...seen];
}

module.exports = { renderTemplate, extractVariables };
