import type { EvalCase, EvalResult, Category, RunOptions } from "./lib/types.ts";
import { evaluate } from "./lib/judge.ts";
import {
  printResult,
  printCategoryHeader,
  computeSummary,
  printSummary,
  printResultsJson,
} from "./lib/reporter.ts";
import { cases as skillLoadingCases } from "./cases/skill-loading.ts";
import { cases as skillSelectionCases } from "./cases/skill-selection.ts";
import { cases as commandUsageCases } from "./cases/command-usage.ts";

const ALL_CASES: EvalCase[] = [
  ...skillLoadingCases,
  ...skillSelectionCases,
  ...commandUsageCases,
];

function parseArgs(args: string[]): RunOptions {
  const options: RunOptions = {
    model: "anthropic/claude-sonnet-4.6",
    judge: false,
    json: false,
    concurrency: 1,
    timeout: 60_000,
  };

  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    switch (arg) {
      case "--model":
        options.model = args[++i] ?? "anthropic/claude-sonnet-4.6";
        break;
      case "--category":
        options.category = args[++i] as Category;
        break;
      case "--judge":
        options.judge = true;
        break;
      case "--json":
        options.json = true;
        break;
      case "--timeout":
        options.timeout = parseInt(args[++i] ?? "60000", 10);
        break;
      case "--help":
      case "-h":
        printUsage();
        process.exit(0);
    }
  }

  return options;
}

function printUsage(): void {
  console.log(`
agent-browser skills evals

Usage: bun run evals/run.ts [options]

Options:
  --model <name>       Model to use via AI Gateway (default: anthropic/claude-sonnet-4.6)
  --category <cat>     Filter by category: skill-loading, skill-selection, command-usage
  --judge              Enable LLM judge for quality scoring (costs extra API calls)
  --json               Output results as JSON
  --timeout <ms>       Timeout per eval case in milliseconds (default: 60000)
  --help, -h           Show this help
`.trim());
}

async function main(): Promise<void> {
  const options = parseArgs(process.argv.slice(2));

  let cases = ALL_CASES;
  if (options.category) {
    cases = cases.filter((c) => c.category === options.category);
  }

  if (cases.length === 0) {
    console.error("No eval cases match the given filters.");
    process.exit(1);
  }

  if (!options.json) {
    console.log(
      `\nRunning ${cases.length} eval(s) with model=${options.model}` +
        (options.judge ? " + LLM judge" : ""),
    );
  }

  const results: EvalResult[] = [];
  const startTime = performance.now();
  let currentCategory: string | null = null;

  for (const evalCase of cases) {
    if (!options.json && evalCase.category !== currentCategory) {
      currentCategory = evalCase.category;
      printCategoryHeader(currentCategory);
    }

    const result = await evaluate(evalCase, {
      model: options.model,
      judge: options.judge,
      timeout: options.timeout,
    });

    results.push(result);

    if (!options.json) {
      printResult(result);
    }
  }

  const totalDurationMs = Math.round(performance.now() - startTime);
  const summary = computeSummary(results, totalDurationMs);

  if (options.json) {
    printResultsJson(results, summary);
  } else {
    printSummary(summary);
  }

  const exitCode = summary.failed > 0 || summary.errors > 0 ? 1 : 0;
  process.exit(exitCode);
}

main();
