# Review decisions

Findings that a review round found **accurate** and that were deliberately not
fixed, because the fix costs more than the defect. `review-time` reads this file
before it reports, so an entry here is out of scope for later reviews.

Reopening is cheap: a real occurrence of the accepted risk reopens a decision. A
new argument about the same hypothetical does not.

Each record below is one file in this directory, named after the finding it
settles. `address-issues` writes them, and `review-time` reads every one of
them before it reviews anything.

One record per file is the whole point of the directory. A single appended list
gave every branch the same anchor to append to, and git cannot order two
additions at one anchor — so it declared a conflict on every merge and every
rebase, over entries whose order carries no meaning.

`README.md` is not a record. Every other `*.md` file here is one.
