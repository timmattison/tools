#!/usr/bin/env -S npx tsx
/**
 * record-github-strikes — records where GitHub strikes text through.
 *
 * `wn` reads the `Blocked by` section of an issue, and an author strikes a
 * blocker through, as in `~~#21~~`, to take it back. So the strike rule of
 * `wn` (`src/wn/src/strike.rs`) must strike through exactly where GitHub does.
 * A rule that only looks like the rule of GitHub fails on the cases nobody
 * thought of. This script asks GitHub instead.
 *
 * The script makes a corpus of one-line cases. The seed cases come first, and
 * 1500 random cases follow. A PRNG with a fixed seed makes the random cases, so
 * each run sends the same cases. The script sends the cases to the Markdown API
 * of GitHub and writes `src/wn/fixtures/github-strikes.tsv`. Each line of that
 * file is a case, a tab, and the HTML inside the paragraph that GitHub made of
 * the case. A test in `strike.rs` holds the rule to that file.
 *
 * The script does not guess. It stops with an error, and it writes nothing,
 * when the answer of GitHub is not one paragraph of text and `<del>` tags for
 * each case.
 *
 * Usage, from any directory:
 *
 *   src/wn/scripts/record-github-strikes.ts
 *
 * It needs `gh`, logged in to github.com (see `gh auth status`). It calls
 * `gh api markdown` once for each batch of cases.
 */

import { execFileSync } from "node:child_process";
import { realpathSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

/** The mark that opens and closes a span struck through. */
const TILDE = "~";

/** Three tildes at the start of a line open a code fence. */
const FENCE_OPEN = TILDE.repeat(3);

/** An ATX heading: one to six `#`, then a space or the end of the line. */
const ATX_HEADING = /^#{1,6}(?: |$)/u;

/** An ordered list item: digits, `.` or `)`, then a space or the end. */
const ORDERED_LIST_ITEM = /^\d+[.)](?: |$)/u;

/**
 * The spaces that GitHub trims from the start and the end of a paragraph, or
 * that change the block a line makes. A case does not start or end with one.
 */
const EDGE_SPACES: readonly string[] = [" ", " ", "　", " "];

/** The characters that separate the fields and the lines of the fixture. */
const FIELD_SEPARATOR = "\t";
const LINE_END = "\n";
const CARRIAGE_RETURN = "\r";

/** A blank line ends a paragraph, so each case is one paragraph. */
const CASE_SEPARATOR = LINE_END + LINE_END;

/** The HTML around the rendering of one case. */
const PARAGRAPH_OPEN = "<p>";
const PARAGRAPH_CLOSE = "</p>";

/** The only tags a rendering can hold. */
const DEL_OPEN = "<del>";
const DEL_CLOSE = "</del>";

/** The characters that start and end a tag. */
const TAG_MARKS = /[<>]/u;

/** The character that starts an HTML entity. */
const ENTITY_MARK = "&";

/** The command and the arguments that render Markdown through GitHub. */
const GH_COMMAND = "gh";
const GH_ARGUMENTS: readonly string[] = ["api", "markdown", "--input", "-"];

/** The mode of the Markdown API that renders plain Markdown, with no links to issues. */
const RENDER_MODE = "markdown";

/** The most cases in one request to GitHub. */
const BATCH_SIZE = 500;

/** The most bytes the script reads from one answer of GitHub. */
const MAX_ANSWER_BYTES = 64 * 1024 * 1024;

/** The number of random cases in the corpus, after the seeds. */
const RANDOM_CASE_COUNT = 1500;

/** The seed of the PRNG. The same seed gives the same random cases. */
const PRNG_SEED = 0x7e1de;

/**
 * The most random texts the script makes before it stops. A corpus that cannot
 * fill in this many attempts is an error, not a wait.
 */
const MAX_RANDOM_ATTEMPTS = 1_000_000;

/** The fewest and the most characters (code points) in a random case. */
const MIN_RANDOM_LENGTH = 2;
const MAX_RANDOM_LENGTH = 16;

/** The fixture, relative to the directory of this script. */
const FIXTURE_RELATIVE_PATH: readonly string[] = ["..", "fixtures", "github-strikes.tsv"];

/**
 * The characters that stand beside a pair of tildes in the flanking seeds:
 * punctuation, symbols, and space characters from outside ASCII.
 */
const FLANKING_CHARACTERS: readonly string[] = [
  "\u{2014}",
  "\u{20AC}",
  "\u{00A9}",
  "\u{201C}",
  "$",
  "\u{1F6A7}",
  "\u{00A0}",
  "\u{00B7}",
  "\u{2028}",
  "\u{3000}",
  "\u{FF01}",
  "\u{2E4F}",
  "\u{1680}",
  "\u{202F}",
  "\u{205F}",
  "\u{2000}",
  "\u{200A}",
  "\u{FF1F}",
];

/** The cases that come first. Each one shows a class of divergence from GitHub. */
const SEED_CASES: readonly string[] = [
  "~~#21~#22~~ #23",
  "~~#21 a~ b~~ #22",
  "~~#21 a ~b~~ #22",
  "~~#21 a~b~~ #22",
  "~~#21 ~~b~~ #22",
  "~~#21 a ~b~~ c~~ #22",
  "#20~~#21~~ #22",
  "\u{1F6A7}~~#21~~ #22",
  "~~#21 (x)~~and #22",
  "~~#21 ~~~ b~~ #22",
  "a~b~ c",
  "~a~~b~ c",
  "~~a~b~ c~~ d",
  "x ~~a~~b~~ c",
  "~~a~ b~~ c~~ d",
  "~~a ~b~~ c~ d",
  "(~~a)~~ b",
  "~~a.~~b",
  "a~~.b~~ c",
  "~~#21 ~~ #22",
  "~~#21 ~~ #22~~ #23",
  "~a~~b c~~ d~~ e~ f",
  "~~#21~~ #22",
  "~#21~",
  "~~#21 ~~ #22",
  "~ #21~ #22",
  "~~#21 #22",
  ...FLANKING_CHARACTERS.flatMap((character) => [
    `a~~${character}b~~ c`,
    `~~b${character}~~a c`,
  ]),
];

/** One character of the random alphabet, and how often it comes. */
interface WeightedCharacter {
  readonly character: string;
  readonly weight: number;
}

/** The alphabet of the random cases. Tildes come most often. */
const RANDOM_ALPHABET: readonly WeightedCharacter[] = [
  { character: TILDE, weight: 6 },
  { character: "a", weight: 2 },
  { character: "b", weight: 2 },
  { character: " ", weight: 3 },
  { character: "#", weight: 1 },
  { character: "1", weight: 1 },
  { character: ".", weight: 1 },
  { character: "(", weight: 1 },
  { character: ")", weight: 1 },
  { character: "$", weight: 1 },
  { character: "\u{2014}", weight: 1 },
  { character: "\u{FF01}", weight: 1 },
  { character: "\u{00B7}", weight: 1 },
  { character: "\u{20AC}", weight: 1 },
  { character: "\u{1F6A7}", weight: 1 },
  { character: "\u{2E4F}", weight: 1 },
  { character: "\u{00A0}", weight: 1 },
  { character: "\u{3000}", weight: 1 },
  { character: "\u{2028}", weight: 1 },
];

/** A text that GitHub renders as one paragraph, and that holds a tilde. */
type Case = string & { readonly __brand: "Case" };

/** The HTML inside the paragraph of one case: text and `<del>` tags only. */
type Rendering = string & { readonly __brand: "Rendering" };

/** A source of random numbers in [0, 1). */
type Random = () => number;

/**
 * A PRNG (mulberry32) that gives the same numbers for the same seed.
 *
 * @param seed - The 32-bit seed.
 * @returns A function that gives the next number in [0, 1).
 */
function mulberry32(seed: number): Random {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let mixed = state;
    mixed = Math.imul(mixed ^ (mixed >>> 15), mixed | 1);
    mixed ^= mixed + Math.imul(mixed ^ (mixed >>> 7), mixed | 61);
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Why `text` is not a case, or `undefined` when it is one.
 *
 * A case holds a tilde, and GitHub renders it as one paragraph that holds all
 * of its characters. So a case does not start or end with a space that GitHub
 * trims, and it does not open a code fence, a heading, or a list item. It holds
 * no tab and no line end, because the fixture separates its fields and its lines
 * with those.
 *
 * @param text - The candidate text.
 * @returns The reason, or `undefined`.
 */
function rejection(text: string): string | undefined {
  const characters = Array.from(text);
  const first = characters[0];
  const last = characters[characters.length - 1];
  if (first === undefined || last === undefined) {
    return "it is empty";
  }
  if (!text.includes(TILDE)) {
    return "it holds no tilde";
  }
  if ([FIELD_SEPARATOR, LINE_END, CARRIAGE_RETURN].some((mark) => text.includes(mark))) {
    return "it holds a tab or a line end";
  }
  if (EDGE_SPACES.includes(first) || EDGE_SPACES.includes(last)) {
    return "it starts or ends with a space that GitHub trims";
  }
  if (text.startsWith(FENCE_OPEN)) {
    return "it opens a code fence";
  }
  if (ATX_HEADING.test(text)) {
    return "it opens a heading";
  }
  if (ORDERED_LIST_ITEM.test(text)) {
    return "it opens a list item";
  }
  return undefined;
}

/**
 * One character of the random alphabet, chosen by weight.
 *
 * @param random - The source of random numbers.
 * @returns The character.
 */
function randomCharacter(random: Random): string {
  const total = RANDOM_ALPHABET.reduce((sum, entry) => sum + entry.weight, 0);
  let remaining = random() * total;
  for (const entry of RANDOM_ALPHABET) {
    remaining -= entry.weight;
    if (remaining < 0) {
      return entry.character;
    }
  }
  const last = RANDOM_ALPHABET[RANDOM_ALPHABET.length - 1];
  if (last === undefined) {
    throw new Error("The random alphabet is empty.");
  }
  return last.character;
}

/**
 * A random text of MIN_RANDOM_LENGTH to MAX_RANDOM_LENGTH code points.
 *
 * @param random - The source of random numbers.
 * @returns The text. It is not always a case.
 */
function randomText(random: Random): string {
  const span = MAX_RANDOM_LENGTH - MIN_RANDOM_LENGTH + 1;
  const length = MIN_RANDOM_LENGTH + Math.floor(random() * span);
  let text = "";
  for (let index = 0; index < length; index += 1) {
    text += randomCharacter(random);
  }
  return text;
}

/**
 * The corpus: the seed cases, then RANDOM_CASE_COUNT random cases, each case
 * one time only.
 *
 * @returns The cases, in the order of the fixture.
 * @throws When a seed is not a case, or when the random cases do not fill.
 */
function corpus(): Case[] {
  const cases: Case[] = [];
  const seen = new Set<string>();
  for (const seed of SEED_CASES) {
    const reason = rejection(seed);
    if (reason !== undefined) {
      throw new Error(`The seed ${JSON.stringify(seed)} is not a case: ${reason}.`);
    }
    if (seen.has(seed)) {
      console.warn(`The seed ${JSON.stringify(seed)} repeats an earlier seed. The corpus keeps the first.`);
      continue;
    }
    seen.add(seed);
    cases.push(seed as Case);
  }

  const random = mulberry32(PRNG_SEED);
  let added = 0;
  for (let attempt = 0; added < RANDOM_CASE_COUNT; attempt += 1) {
    if (attempt >= MAX_RANDOM_ATTEMPTS) {
      throw new Error(
        `The script made ${MAX_RANDOM_ATTEMPTS} random texts and found only ${added} new cases.`,
      );
    }
    const text = randomText(random);
    if (rejection(text) !== undefined || seen.has(text)) {
      continue;
    }
    seen.add(text);
    cases.push(text as Case);
    added += 1;
  }
  return cases;
}

/**
 * The rendering of one case, taken from one line of the answer of GitHub.
 *
 * @param source - The case.
 * @param line - The line of the answer for that case.
 * @returns The HTML inside the paragraph.
 * @throws When the line is not a paragraph of text and `<del>` tags, or when
 *   its text is not the text of the case.
 */
function renderingOf(source: Case, line: string): Rendering {
  const name = JSON.stringify(source);
  if (!line.startsWith(PARAGRAPH_OPEN) || !line.endsWith(PARAGRAPH_CLOSE)) {
    throw new Error(`GitHub rendered the case ${name} as ${JSON.stringify(line)}, which is not one paragraph.`);
  }
  const inner = line.slice(PARAGRAPH_OPEN.length, line.length - PARAGRAPH_CLOSE.length);
  const text = inner.replaceAll(DEL_OPEN, "").replaceAll(DEL_CLOSE, "");
  if (TAG_MARKS.test(text)) {
    throw new Error(`GitHub rendered the case ${name} as ${JSON.stringify(line)}, which holds a tag other than <del>.`);
  }
  if (inner.includes(ENTITY_MARK)) {
    throw new Error(`GitHub rendered the case ${name} as ${JSON.stringify(line)}, which holds an entity.`);
  }
  if (text.replaceAll(TILDE, "") !== source.replaceAll(TILDE, "")) {
    throw new Error(
      `GitHub rendered the case ${name} as ${JSON.stringify(line)}, whose text is not the text of the case. ` +
        "The lines of the answer do not align with the cases.",
    );
  }
  return inner as Rendering;
}

/**
 * The renderings of one batch of cases, from one request to GitHub.
 *
 * @param batch - The cases. GitHub renders each one as one paragraph on one line.
 * @returns The rendering of each case, in the order of the batch.
 * @throws When `gh` fails, or when the answer does not hold one paragraph for
 *   each case.
 */
function renderBatch(batch: readonly Case[]): Rendering[] {
  const request = JSON.stringify({ text: batch.join(CASE_SEPARATOR), mode: RENDER_MODE });
  const answer = execFileSync(GH_COMMAND, GH_ARGUMENTS, {
    input: request,
    encoding: "utf8",
    maxBuffer: MAX_ANSWER_BYTES,
    stdio: ["pipe", "pipe", "inherit"],
  });
  const body = answer.endsWith(LINE_END) ? answer.slice(0, -LINE_END.length) : answer;
  const lines = body.split(LINE_END);
  if (lines.length !== batch.length) {
    throw new Error(
      `GitHub gave ${lines.length} lines for a batch of ${batch.length} cases, ` +
        `from the case ${JSON.stringify(batch[0])} to the case ${JSON.stringify(batch[batch.length - 1])}.`,
    );
  }
  return batch.map((source, index) => {
    const line = lines[index];
    if (line === undefined) {
      throw new Error(`GitHub gave no line for the case ${JSON.stringify(source)}.`);
    }
    return renderingOf(source, line);
  });
}

/**
 * The path of the fixture, found from the path of this script, so the script
 * runs from any directory.
 *
 * @returns The absolute path.
 * @throws When the process names no script.
 */
function fixturePath(): string {
  const script = process.argv[1];
  if (script === undefined) {
    throw new Error("The process names no script, so the script cannot find the fixture.");
  }
  return join(dirname(realpathSync(script)), ...FIXTURE_RELATIVE_PATH);
}

/** Records the corpus and writes the fixture. */
function main(): void {
  const path = fixturePath();
  const cases = corpus();
  const lines: string[] = [];
  for (let start = 0; start < cases.length; start += BATCH_SIZE) {
    const batch = cases.slice(start, start + BATCH_SIZE);
    const renderings = renderBatch(batch);
    batch.forEach((source, index) => {
      lines.push(source + FIELD_SEPARATOR + renderings[index] + LINE_END);
    });
    console.log(`GitHub rendered ${start + batch.length} of ${cases.length} cases.`);
  }
  writeFileSync(path, lines.join(""));
  console.log(`Wrote ${cases.length} cases to ${path}.`);
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
