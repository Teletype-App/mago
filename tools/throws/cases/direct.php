<?php
declare(strict_types=1);
namespace ThrowsProbe\direct;
function direct(): void { throw new \DomainException(); }
