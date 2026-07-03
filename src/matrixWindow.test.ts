import { describe, expect, it } from "vitest";
import { computeMatrixWindow } from "./matrixWindow";

const BASE = {
  scrollTop: 0,
  scrollLeft: 0,
  viewportHeight: 800,
  viewportWidth: 1200,
  headerHeight: 240,
  leftWidth: 1000,
  rowHeight: 40,
  colWidth: 120,
  rowCount: 10_000,
  colCount: 500,
  overscanRows: 10,
  overscanCols: 4
};

describe("computeMatrixWindow", () => {
  it("starts at the origin with a viewport-sized window plus overscan", () => {
    const win = computeMatrixWindow(BASE);
    expect(win.rowStart).toBe(0);
    expect(win.colStart).toBe(0);
    // ceil(800/40)+1 visible +10 overscan = 31
    expect(win.rowEnd).toBe(31);
    // ceil(1200/120)+1 visible +4 overscan = 15
    expect(win.colEnd).toBe(15);
  });

  it("offsets rows by the header height and applies overscan on both sides", () => {
    const win = computeMatrixWindow({ ...BASE, scrollTop: 240 + 40 * 100 });
    // first visible row is 100 → overscan pulls start back to 90
    expect(win.rowStart).toBe(90);
    expect(win.rowEnd).toBe(100 + 21 + 10);
  });

  it("treats the left info columns as a horizontal header offset", () => {
    // scrolled less than the left-column block → still at column 0
    expect(computeMatrixWindow({ ...BASE, scrollLeft: 600 }).colStart).toBe(0);
    const win = computeMatrixWindow({ ...BASE, scrollLeft: 1000 + 120 * 50 });
    expect(win.colStart).toBe(46);
    expect(win.colEnd).toBe(50 + 11 + 4);
  });

  it("clamps to the loaded row/column counts near the end", () => {
    const win = computeMatrixWindow({
      ...BASE,
      rowCount: 105,
      colCount: 12,
      scrollTop: 240 + 40 * 100,
      scrollLeft: 1000 + 120 * 10
    });
    expect(win.rowEnd).toBe(105);
    expect(win.rowStart).toBeLessThanOrEqual(105);
    expect(win.colEnd).toBe(12);
    expect(win.colStart).toBeLessThanOrEqual(12);
    expect(win.rowStart).toBeLessThanOrEqual(win.rowEnd);
    expect(win.colStart).toBeLessThanOrEqual(win.colEnd);
  });

  it("renders a bounded non-empty window when the viewport has no size yet", () => {
    // jsdom / first paint before layout: clientHeight/clientWidth are 0.
    const win = computeMatrixWindow({ ...BASE, viewportHeight: 0, viewportWidth: 0 });
    expect(win.rowEnd).toBeGreaterThan(0);
    expect(win.colEnd).toBeGreaterThan(0);
    expect(win.rowEnd).toBeLessThanOrEqual(60);
    expect(win.colEnd).toBeLessThanOrEqual(30);
  });

  it("never lets the window start past the end of the data", () => {
    const win = computeMatrixWindow({ ...BASE, rowCount: 5, scrollTop: 240 + 40 * 1000 });
    expect(win.rowStart).toBeLessThanOrEqual(5);
    expect(win.rowEnd).toBe(5);
  });

  describe("variable column widths (colOffsets)", () => {
    // Deliberately non-uniform widths (400, 50, 50, 50, 400) so uniform-width
    // math would land on different columns than offset math.
    const OFFSETS = [0, 400, 450, 500, 550, 950];
    const VAR = { ...BASE, colCount: 5, colOffsets: OFFSETS, overscanCols: 0 };

    it("starts at the first column when the left block is still in view", () => {
      const win = computeMatrixWindow({ ...VAR, scrollLeft: 0 });
      expect(win.colStart).toBe(0);
    });

    it("locates the visible window by offsets, not by uniform width", () => {
      // Visible span starts 420px into the grid → inside column 1 (400..450);
      // with a 60px viewport it ends inside column 2 (450..500).
      // Uniform 120px math would have said columns 3..5.
      const win = computeMatrixWindow({
        ...VAR,
        viewportWidth: 60,
        scrollLeft: 1000 + 420
      });
      expect(win.colStart).toBe(1);
      expect(win.colEnd).toBe(3);
    });

    it("spans many narrow columns when the viewport is wide", () => {
      // 420..900 covers columns 1,2,3 and reaches into 4 (550..950).
      const win = computeMatrixWindow({
        ...VAR,
        viewportWidth: 480,
        scrollLeft: 1000 + 420
      });
      expect(win.colStart).toBe(1);
      expect(win.colEnd).toBe(5);
    });

    it("applies column overscan on both sides", () => {
      const win = computeMatrixWindow({
        ...VAR,
        overscanCols: 1,
        viewportWidth: 60,
        scrollLeft: 1000 + 420
      });
      expect(win.colStart).toBe(0);
      expect(win.colEnd).toBe(4);
    });

    it("clamps to the column count when scrolled past the end", () => {
      const win = computeMatrixWindow({ ...VAR, scrollLeft: 1000 + 5000 });
      expect(win.colStart).toBeLessThanOrEqual(5);
      expect(win.colEnd).toBe(5);
      expect(win.colStart).toBeLessThanOrEqual(win.colEnd);
    });
  });
});
