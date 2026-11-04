import { Fragment } from "react";

/** Metric name that prefers wrapping after its `/`, `.` and `_` separators over splitting a word. */
export function MetricName({ name }: { name: string }) {
  return name.split(/(?<=[/._])/).map((part, index) => (
    <Fragment key={index}>{part}<wbr /></Fragment>
  ));
}
