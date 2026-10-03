<?php
declare(strict_types=1);
namespace ThrowsProbe\catch_parent;
/** @throws \DomainException */
function leaf(): void { throw new \DomainException(); }
function caller(): void { try { leaf(); } catch (\LogicException $e) {} }
