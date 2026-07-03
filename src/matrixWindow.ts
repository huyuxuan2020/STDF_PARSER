// Pure windowing math for the virtualized test-item matrix. The table renders
// only the rows/columns inside this window; everything outside is replaced by
// fixed-size spacers so scroll geometry stays identical to a full render.

export interface MatrixWindow {
  rowStart: number;
  rowEnd: number;
  colStart: number;
  colEnd: number;
}

// Fallback viewport used before the first layout (and in jsdom, where client
// sizes are always 0) so the initial paint still shows a sensible window.
const FALLBACK_VIEWPORT_HEIGHT = 900;
const FALLBACK_VIEWPORT_WIDTH = 1600;

export function computeMatrixWindow(params: {
  scrollTop: number;
  scrollLeft: number;
  viewportHeight: number;
  viewportWidth: number;
  /** Rendered height of the table header block above the first data row. */
  headerHeight: number;
  /** Total width of the per-part info columns to the left of the value grid. */
  leftWidth: number;
  rowHeight: number;
  /** Uniform column width; ignored when `colOffsets` is provided. */
  colWidth: number;
  rowCount: number;
  colCount: number;
  /**
   * Prefix sums of per-column widths (length colCount + 1, starting at 0) for
   * auto-fitted variable-width columns. When present, the column window is
   * located by binary search over these offsets.
   */
  colOffsets?: number[];
  overscanRows?: number;
  overscanCols?: number;
}): MatrixWindow {
  const {
    scrollTop,
    scrollLeft,
    headerHeight,
    leftWidth,
    rowHeight,
    colWidth,
    rowCount,
    colCount,
    colOffsets,
    overscanRows = 10,
    overscanCols = 4
  } = params;
  const viewportHeight = params.viewportHeight > 0 ? params.viewportHeight : FALLBACK_VIEWPORT_HEIGHT;
  const viewportWidth = params.viewportWidth > 0 ? params.viewportWidth : FALLBACK_VIEWPORT_WIDTH;

  const firstRow = Math.floor(Math.max(0, scrollTop - headerHeight) / rowHeight);
  const visibleRows = Math.ceil(viewportHeight / rowHeight) + 1;
  const rowStart = Math.min(Math.max(0, firstRow - overscanRows), rowCount);
  const rowEnd = Math.min(rowCount, firstRow + visibleRows + overscanRows);

  const gridLeft = Math.max(0, scrollLeft - leftWidth);
  let firstCol: number;
  let lastCol: number; // exclusive
  if (colOffsets && colOffsets.length === colCount + 1) {
    firstCol = lastColumnStartingAtOrBefore(colOffsets, gridLeft);
    lastCol = Math.max(firstCol + 1, lastColumnStartingAtOrBefore(colOffsets, gridLeft + viewportWidth) + 1);
  } else {
    firstCol = Math.floor(gridLeft / colWidth);
    lastCol = firstCol + Math.ceil(viewportWidth / colWidth) + 1;
  }
  const colStart = Math.min(Math.max(0, firstCol - overscanCols), colCount);
  const colEnd = Math.min(colCount, lastCol + overscanCols);

  return { rowStart, rowEnd: Math.max(rowStart, rowEnd), colStart, colEnd: Math.max(colStart, colEnd) };
}

/** Index of the last column whose left edge is at or before `x` (binary search). */
function lastColumnStartingAtOrBefore(offsets: number[], x: number): number {
  let lo = 0;
  let hi = offsets.length - 2; // last valid column index
  if (hi < 0) return 0;
  if (x <= 0) return 0;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (offsets[mid] <= x) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}
