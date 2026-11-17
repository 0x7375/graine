import * as React from "react";
import { cn } from "@/lib/utils";

type TableProps = React.HTMLAttributes<HTMLTableElement> & {
  containerClassName?: string;
  visibleRows?: number;
};

const Table = React.forwardRef<HTMLTableElement, TableProps>(
  ({ className, containerClassName, visibleRows, ...props }, forwardedRef) => {
    const internalRef = React.useRef<HTMLTableElement>(null);
    const [maxHeight, setMaxHeight] = React.useState<number | undefined>();

    // Measure ten complete rows plus part of the next row to suggest scrolling.
    React.useLayoutEffect(() => {
      const table = internalRef.current;
      if (!table || !visibleRows) return;
      const header = table.querySelector("thead");
      const completeRows = Math.floor(visibleRows);
      const partialRow = visibleRows - completeRows;
      const rows = Array.from(table.querySelectorAll("tbody > tr:not([data-ranking-details])"));
      const measure = () => {
        if (rows.length <= completeRows) {
          setMaxHeight((previous) => previous === undefined ? previous : undefined);
          return;
        }
        // Sum the actual heights of the visible primary rows, not their viewport
        // positions: expanded detail rows must remain inside a fixed scroll area.
        const headerHeight = header?.getBoundingClientRect().height ?? 0;
        const completeHeight = rows.slice(0, completeRows).reduce(
          (sum, row) => sum + row.getBoundingClientRect().height, 0,
        );
        const nextHeight = rows[completeRows]?.getBoundingClientRect().height ?? 0;
        const height = Math.ceil(headerHeight + completeHeight + nextHeight * partialRow);
        setMaxHeight((previous) => previous === height ? previous : height);
      };
      measure();
      const observer = new ResizeObserver(measure);
      if (header) observer.observe(header);
      rows.slice(0, visibleRows).forEach((row) => observer.observe(row));
      return () => observer.disconnect();
    });

    const setRef = (element: HTMLTableElement | null) => {
      internalRef.current = element;
      if (typeof forwardedRef === "function") forwardedRef(element);
      else if (forwardedRef) forwardedRef.current = element;
    };

    return (
      <div
        className={cn("relative w-full overflow-auto", containerClassName)}
        style={{ maxHeight, overflowAnchor: "none" }}
      >
        <table ref={setRef} className={cn("w-full caption-bottom text-sm", className)} {...props} />
      </div>
    );
  },
);
Table.displayName = "Table";

const TableHeader = React.forwardRef<HTMLTableSectionElement, React.HTMLAttributes<HTMLTableSectionElement>>(
  ({ className, ...props }, ref) => <thead ref={ref} className={cn("[&_tr]:border-b", className)} {...props} />,
);
TableHeader.displayName = "TableHeader";

const TableBody = React.forwardRef<HTMLTableSectionElement, React.HTMLAttributes<HTMLTableSectionElement>>(
  ({ className, ...props }, ref) => <tbody ref={ref} className={cn("[&_tr:last-child]:border-0", className)} {...props} />,
);
TableBody.displayName = "TableBody";

const TableFooter = React.forwardRef<HTMLTableSectionElement, React.HTMLAttributes<HTMLTableSectionElement>>(
  ({ className, ...props }, ref) => <tfoot ref={ref} className={cn("border-t bg-muted/50 font-medium [&>tr]:last:border-b-0", className)} {...props} />,
);
TableFooter.displayName = "TableFooter";

const TableRow = React.forwardRef<HTMLTableRowElement, React.HTMLAttributes<HTMLTableRowElement>>(
  ({ className, ...props }, ref) => (
    <tr ref={ref} className={cn("border-b transition-colors hover:bg-muted/40 data-[state=selected]:bg-muted", className)} {...props} />
  ),
);
TableRow.displayName = "TableRow";

const TableHead = React.forwardRef<HTMLTableCellElement, React.ThHTMLAttributes<HTMLTableCellElement>>(
  ({ className, ...props }, ref) => (
    <th ref={ref} className={cn("h-10 px-3 text-left align-middle text-xs font-medium uppercase tracking-wide text-muted-foreground [&:has([role=checkbox])]:pr-0", className)} {...props} />
  ),
);
TableHead.displayName = "TableHead";

const TableCell = React.forwardRef<HTMLTableCellElement, React.TdHTMLAttributes<HTMLTableCellElement>>(
  ({ className, ...props }, ref) => (
    <td ref={ref} className={cn("px-3 py-2.5 align-middle [&:has([role=checkbox])]:pr-0", className)} {...props} />
  ),
);
TableCell.displayName = "TableCell";

const TableCaption = React.forwardRef<HTMLTableCaptionElement, React.HTMLAttributes<HTMLTableCaptionElement>>(
  ({ className, ...props }, ref) => <caption ref={ref} className={cn("mt-4 text-sm text-muted-foreground", className)} {...props} />,
);
TableCaption.displayName = "TableCaption";

export { Table, TableHeader, TableBody, TableFooter, TableHead, TableRow, TableCell, TableCaption };
