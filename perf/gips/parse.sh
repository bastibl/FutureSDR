#!/bin/bash

outfile=perf-data/results.csv
rm -f ${outfile}

echo "sdr,run,pipes,stages,samples,max_copy,scheduler,time" > ${outfile}

files=$(ls perf-data/gr_*.csv 2>/dev/null || echo)
for f in ${files}
do
	echo "gr,$(cat $f)" >> ${outfile}
done

files=$(ls perf-data/fs_*.csv 2>/dev/null || echo)
for f in ${files}
do
	echo "fs,$(cat $f)" >> ${outfile}
done
