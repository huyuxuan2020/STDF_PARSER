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
  colWidth: number;
  rowCount: number;
  colCount: number;
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
    overscanRows = 10,
    overscanCols = 4
  } = params;
  const viewportHeight = params.viewportHeight > 0 ? params.viewportHeight : FALLBACK_VIEWPORT_HEIGHT;
  const viewportWidth = params.viewportWidth > 0 ? params.viewportWidth : FALLBACK_VIEWPORT_WIDTH;

  const firstRow = Math.floor(Math.max(0, scrollTop - headerHeight) / rowHeight);
  const visibleRows = Math.ceil(viewportHeight / rowHeight) + 1;
  const rowStart = Math.min(Math.max(0, firstRow - overscanRows), rowCount);
  const rowEnd = Math.min(rowCount, firstRow + visibleRows + overscanRows);

  const firstCol = Math.floor(Math.max(0, scrollLeft - leftWidth) / colWidth);
  const visibleCols = Math.ceil(viewportWidth / colWidth) + 1;
  const colStart = Math.min(Math.max(0, firstCol - overscanCols), colCount);
  const colEnd = Math.min(colCount, firstCol + visibleCols + overscanCols);

  return { rowStart, rowEnd: Math.max(rowStart, rowEnd), colStart, colEnd: Math.max(colStart, colEnd) };
}
