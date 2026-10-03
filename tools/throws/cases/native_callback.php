<?php
declare(strict_types=1);
namespace ThrowsProbe\native_callback;
function caller(): void { array_map(static function (int $value): int { throw new \DomainException(); }, [1]); }
function emptyArray(): void { array_map(static function (int $value): int { throw new \DomainException(); }, []); }
function multipleArrays(): void { array_map(static function (?int $left, int $right): int { throw new \DomainException(); }, [], [1]); }
function emptyArrays(): void { array_map(static function (?int $left, ?int $right): int { throw new \DomainException(); }, [], []); }
function deferred(): void { register_shutdown_function(static function (): void { throw new \DomainException(); }); }
