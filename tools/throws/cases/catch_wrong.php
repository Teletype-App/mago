<?php
declare(strict_types=1);
namespace ThrowsProbe\catch_wrong;
/** @throws \DomainException */
function leaf(): void { throw new \DomainException(); }
function caller(): void { try { leaf(); } catch (\RuntimeException $e) {} }
