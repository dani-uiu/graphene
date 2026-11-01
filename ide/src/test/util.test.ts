import { describe, expect, it } from "vitest";
import { paletteColor } from "../graph";
import { baseName, dirName, fuzzy, short, valueText } from "../util";

describe("util", () => {
  it("shows list values with their length", () => {
    expect(valueText([1, 2, 3])).toBe("[3] [1,2,3]");
    expect(valueText({ a: 1 })).toBe(short({ a: 1 }));
    expect(valueText(Array(50).fill(7), 20).startsWith("[50] [7,7")).toBe(true);
  });
  it("fuzzy matches letters in order", () => {
    expect(fuzzy("format document", "fmt")).toBe(true);
    expect(fuzzy("format document", "tmf")).toBe(false);
    expect(fuzzy("anything", "")).toBe(true);
  });

  it("short truncates long values", () => {
    expect(short("abc", 10)).toBe("abc");
    expect(short({ a: "1234567890" }, 8)).toBe("{\"a\":\"1…");
    expect(short(42)).toBe("42");
  });

  it("splits Windows and POSIX paths", () => {
    expect(baseName(["C:", "x", "y", "pricing.grph"].join(String.fromCharCode(92)))).toBe("pricing.grph");
    expect(dirName("/a/b/c.json")).toBe("/a/b");
    expect(dirName("c.json")).toBe("");
  });
});

describe("graph colors", () => {
  it("are stable per key", () => {
    expect(paletteColor("dec")).toBe(paletteColor("dec"));
    expect(paletteColor("always")).toMatch(/^#[0-9a-f]{6}$/);
  });
});
