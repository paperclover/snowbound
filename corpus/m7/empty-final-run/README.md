# Empty final formatting run

`input` is the 24-byte minimized stateful fuzz input retained from the M7 text
editing campaign. The old oracle represented only characters and lost the
insertion style of an emptied final run. An insertion there inherited the
retained final run's style, which disagreed with that incomplete oracle.

The stable semantic replay is
`tests/edit.rs::an_emptied_final_run_retains_its_insertion_style`: erase UTF-16
positions 22..27 in the native formatted fixture, insert `a` at 22, and require
the original final run's formatting. The current fuzz oracle carries an explicit
terminal style marker. Fixture-selection bytes have changed as cases were added;
the named regression is the replay independent of that encoding.
