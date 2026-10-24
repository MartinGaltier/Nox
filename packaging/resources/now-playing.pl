#!/usr/bin/perl
# SPDX-License-Identifier: GPL-3.0-or-later
# Adapted from vorssaint-utils (GPL-3.0-or-later), Resources/now-playing.pl, commit e916961.
#
# Loads the Nox Now Playing adapter (libnox_nowplaying.dylib) into perl and
# runs one entry point. perl is a platform binary, so MediaRemote answers it
# where it no longer answers the app (macOS 15.4+). See crates/nox-nowplaying.
#   now-playing.pl <adapter library> [get|watch]
use strict;
use warnings;
use DynaLoader;

$| = 1;
my ($library, $mode) = @ARGV;
$mode //= "get";
my %entries = map { $_ => "nox_now_playing_$_" } qw(get watch);
die "now-playing: unknown mode\n" unless exists $entries{$mode};
die "usage: now-playing.pl <adapter library> [get|watch]\n" unless defined $library && -f $library;
my $handle = DynaLoader::dl_load_file($library, 0)
    or die "now-playing: cannot load adapter: " . DynaLoader::dl_error() . "\n";
my $symbol = DynaLoader::dl_find_symbol($handle, $entries{$mode})
    or die "now-playing: adapter entry point missing: " . DynaLoader::dl_error() . "\n";
DynaLoader::dl_install_xsub("main::nox_now_playing_run", $symbol);
nox_now_playing_run();
