<?php
declare(strict_types=1);
namespace ThrowsProbe\conditional;
/** @throws \DomainException */
function leaf(bool $raise): void { if ($raise) { throw new \DomainException(); } }
function caller_false(): void { leaf(false); }
function caller_true(): void { leaf(true); }
