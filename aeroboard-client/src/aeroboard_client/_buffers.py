"""Column buffers avoid allocating a dictionary for each observation."""


class ColumnBuffer:
    def __init__(self, width=5):
        self.columns = [[] for _ in range(width)]

    def append(self, *row):
        if len(row) != len(self.columns):
            raise ValueError("buffer row does not match its columns")
        for column, value in zip(self.columns, row):
            column.append(value)

    def __len__(self):
        return len(self.columns[0])

    def clear(self):
        for column in self.columns:
            column.clear()

    def __eq__(self, other):
        return other == [] and not len(self)


class ScalarBuffer(ColumnBuffer):
    """Timestamp, run, metric, value and step columns."""
