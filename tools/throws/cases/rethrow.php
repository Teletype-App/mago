<?php
declare(strict_types=1);
namespace ThrowsProbe\rethrow;
/** @throws \DomainException */
function leaf(): void { throw new \DomainException(); }
function caller(): void { try { leaf(); } catch (\LogicException $e) { throw $e; } }
